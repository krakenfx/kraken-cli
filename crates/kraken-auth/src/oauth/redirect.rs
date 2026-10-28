use super::{
    callback::{AuthCode, CALLBACK_PATH, Callback, serve},
    config::RedirectPorts,
    pkce::State,
};
use crate::error::{Error, Result};
use std::{
    io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};
use tokio::net::{TcpListener, TcpStream};

pub(crate) const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// A silent connection must not block the real redirect from the browser.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// A failed `accept` (a reset preconnect, a full fd table) retries after this
/// pause, so a persistent failure does not spin until the login times out.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(50);
/// Connections are served at once, so an idle one (a browser preconnect)
/// never delays the redirect. Past this many, a new one is dropped.
const MAX_OPEN_CONNECTIONS: usize = 16;

#[derive(Debug)]
pub(crate) struct Listener {
    v4: TcpListener,
    /// `localhost` can resolve to `::1`, so the listener holds the port on IPv4
    /// and IPv6.
    v6: Option<TcpListener>,
    redirect_uri: String,
}

impl Listener {
    /// Uses the first port that is free on IPv4 and on IPv6.
    pub(crate) async fn bind(ports: &RedirectPorts) -> Result<Self> {
        let mut last_error = None;
        for port in ports.iter() {
            match bind_port(port).await {
                Ok(listener) => return Ok(listener),
                Err(error) => {
                    tracing::debug!(port, %error, "redirect port unavailable");
                    last_error = Some(error);
                }
            }
        }
        Err(Error::NoFreePort {
            ports: ports.iter().collect(),
            source: last_error.unwrap_or_else(|| io::Error::other("no redirect ports")),
        })
    }

    pub(crate) fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub(crate) async fn wait(&self, state: &State, timeout: Duration) -> Result<AuthCode> {
        tokio::time::timeout(timeout, self.next_code(state))
            .await
            .map_err(|_| Error::TimedOut(timeout))?
    }

    /// Dropping the set on return closes every other open connection.
    async fn next_code(&self, state: &State) -> Result<AuthCode> {
        let mut open = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = self.accept() => match accepted {
                    Ok((mut stream, _)) if open.len() < MAX_OPEN_CONNECTIONS => {
                        let state = state.clone();
                        open.spawn(async move {
                            tokio::time::timeout(READ_TIMEOUT, serve(&mut stream, &state)).await?
                        });
                    }
                    Ok(_) => tracing::debug!("dropped a connection: too many are open"),
                    Err(error) => {
                        tracing::debug!(%error, "could not accept a connection");
                        tokio::time::sleep(ACCEPT_RETRY_DELAY).await;
                    }
                },
                Some(served) = open.join_next() => match served {
                    Ok(Ok(callback)) => {
                        if let Some(outcome) = conclude(callback) {
                            return outcome;
                        }
                    }
                    // A timeout arrives here as `TimedOut`.
                    Ok(Err(error)) => tracing::debug!(%error, "dropped a request"),
                    Err(error) => tracing::debug!(%error, "a connection task failed"),
                },
            }
        }
    }

    async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        // `accept` is cancel-safe, so the branch that loses the race loses no
        // connection.
        match &self.v6 {
            Some(v6) => tokio::select! {
                accepted = self.v4.accept() => accepted,
                accepted = v6.accept() => accepted,
            },
            None => self.v4.accept().await,
        }
    }
}

/// The end of the sign-in, or `None` to keep waiting.
fn conclude(callback: Callback) -> Option<Result<AuthCode>> {
    match callback {
        Callback::Code(code) => Some(Ok(code)),
        Callback::Denied => Some(Err(Error::Denied)),
        Callback::Failed(code) => Some(Err(Error::AuthorizationFailed(code))),
        Callback::Stray => {
            tracing::warn!("ignored a callback that does not belong to this sign-in");
            None
        }
        Callback::NotFound => None,
    }
}

async fn bind_port(port: u16) -> io::Result<Listener> {
    let v4 = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    let v6 = match TcpListener::bind((Ipv6Addr::LOCALHOST, port)).await {
        Ok(v6) => Some(v6),
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => return Err(error),
        Err(error) => {
            tracing::debug!(port, %error, "no IPv6 loopback; listening on 127.0.0.1 only");
            None
        }
    };
    Ok(Listener {
        v4,
        v6,
        redirect_uri: format!("http://localhost:{port}{CALLBACK_PATH}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn state() -> State {
        State::fixed("s3cr3t-state")
    }

    async fn free_port() -> u16 {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[tokio::test]
    async fn a_taken_port_falls_through_to_the_next() {
        let taken = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let taken_port = taken.local_addr().unwrap().port();
        let free = free_port().await;

        let listener = Listener::bind(&RedirectPorts::new([taken_port, free]).unwrap())
            .await
            .unwrap();
        assert_eq!(
            listener.redirect_uri(),
            format!("http://localhost:{free}/callback")
        );
    }

    async fn listener() -> (Listener, SocketAddr) {
        let listener = Listener::bind(&RedirectPorts::new([free_port().await]).unwrap())
            .await
            .unwrap();
        let addr = listener.v4.local_addr().unwrap();
        (listener, addr)
    }

    /// Acts as the browser: it sends one redirect and reads the page.
    async fn send_callback(addr: SocketAddr, query: &str) {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let request = format!("GET /callback?{query} HTTP/1.1\r\nHost: localhost\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut page = String::new();
        stream.read_to_string(&mut page).await.unwrap();
    }

    #[tokio::test]
    async fn a_stray_callback_does_not_end_the_login() {
        let (listener, addr) = listener().await;
        let browser = tokio::spawn(async move {
            send_callback(addr, "code=forged&state=guessed").await;
            send_callback(addr, "code=real&state=s3cr3t-state").await;
        });

        let code = listener.wait(&state(), LOGIN_TIMEOUT).await.unwrap();
        assert_eq!(code.expose(), "real");
        browser.await.unwrap();
    }

    /// Real time, bounded well below `READ_TIMEOUT`: a paused clock would fire
    /// the idle socket's timeout at once and hide a listener that waits on it.
    #[tokio::test]
    async fn an_idle_connection_never_delays_the_redirect() {
        let (listener, addr) = listener().await;
        let _idle = TcpStream::connect(addr).await.unwrap();
        let browser = tokio::spawn(send_callback(addr, "code=real&state=s3cr3t-state"));

        let code = tokio::time::timeout(READ_TIMEOUT / 2, listener.wait(&state(), LOGIN_TIMEOUT))
            .await
            .expect("the redirect waited behind an idle connection")
            .unwrap();
        assert_eq!(code.expose(), "real");
        browser.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_login_nobody_finishes_times_out() {
        let (listener, _) = listener().await;
        let error = listener.wait(&state(), LOGIN_TIMEOUT).await.unwrap_err();
        assert!(matches!(error, Error::TimedOut(t) if t == LOGIN_TIMEOUT));
    }
}
