//! One redirect request from the browser: read it, decide what it means, and
//! answer with a page. Pure of sockets, so the rules are testable on a stream.
use super::pkce::State;
use secrecy::{ExposeSecret, SecretString};
use std::{collections::HashMap, io};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

pub(super) const CALLBACK_PATH: &str = "/callback";
/// The request line carries the code. A longer one is not a redirect.
const MAX_REQUEST_LINE_BYTES: u64 = 8 * 1024;
/// Browsers send every `localhost` cookie, whatever the port, so another local
/// app's cookies can make the head far larger than the request line.
const MAX_HEAD_BYTES: u64 = 1024 * 1024;

/// A secret until the token exchange uses it.
#[derive(Debug)]
pub(crate) struct AuthCode(SecretString);

impl AuthCode {
    pub(crate) fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

#[derive(Debug)]
pub(super) enum Callback {
    Code(AuthCode),
    Denied,
    Failed(String),
    /// A wrong or missing `state`, or a bad query. The listener ignores it, so
    /// a stray page cannot stop the login.
    Stray,
    /// Any other path, such as the browser's `favicon.ico`.
    NotFound,
}

pub(super) async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    state: &State,
) -> io::Result<Callback> {
    let request_line = read_request_line(stream).await?;
    let callback = parse(&request_line, state);
    // The code is valid even when the browser hangs up before the page.
    if let Err(error) = reply(stream, &callback).await {
        tracing::debug!(%error, "could not send the callback page");
    }
    Ok(callback)
}

async fn reply<S: AsyncWrite + Unpin>(stream: &mut S, callback: &Callback) -> io::Result<()> {
    stream.write_all(response(callback).as_bytes()).await?;
    stream.shutdown().await
}

/// Read the full request head before the reply. Otherwise the socket can reset,
/// and the browser shows an error, not the page. Only the request line is kept.
async fn read_request_line<S: AsyncRead + Unpin>(stream: &mut S) -> io::Result<String> {
    let mut head = BufReader::new(stream).take(MAX_HEAD_BYTES);
    let mut line = Vec::new();
    (&mut head)
        .take(MAX_REQUEST_LINE_BYTES)
        .read_until(b'\n', &mut line)
        .await?;
    let request_line = String::from_utf8_lossy(without_crlf(&line)?).into_owned();
    loop {
        line.clear();
        head.read_until(b'\n', &mut line).await?;
        if without_crlf(&line)?.is_empty() {
            return Ok(request_line);
        }
    }
}

/// A line cut short by the browser hanging up or by a size cap ends the read.
fn without_crlf(line: &[u8]) -> io::Result<&[u8]> {
    line.strip_suffix(b"\r\n").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "incomplete or oversized request head",
        )
    })
}

fn parse(request_line: &str, state: &State) -> Callback {
    let mut parts = request_line.split_whitespace();
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
        return Callback::NotFound;
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != CALLBACK_PATH {
        return Callback::NotFound;
    }

    let mut params = HashMap::new();
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        // A real redirect never repeats a parameter.
        if params.insert(key, value).is_some() {
            return Callback::Stray;
        }
    }
    if params.get("state").map(|s| s.as_ref()) != Some(state.as_str()) {
        return Callback::Stray;
    }
    match (params.get("error"), params.get("code")) {
        (Some(error), _) if *error == "access_denied" => Callback::Denied,
        (Some(error), _) => Callback::Failed(error.to_string()),
        (None, Some(code)) if !code.is_empty() => Callback::Code(AuthCode(code.to_string().into())),
        (None, _) => Callback::Stray,
    }
}

fn response(callback: &Callback) -> String {
    let (status, message) = match callback {
        // The key is not created yet, so the page cannot say "signed in".
        Callback::Code(_) => (
            "200 OK",
            "Approved. Return to the terminal, which finishes the sign-in. You can close this tab.",
        ),
        Callback::Denied => ("200 OK", "Sign-in cancelled. You can close this tab."),
        Callback::Failed(_) => ("200 OK", "Sign-in failed. Check the terminal."),
        Callback::Stray => (
            "400 Bad Request",
            "This link does not belong to the running sign-in.",
        ),
        Callback::NotFound => ("404 Not Found", "Not found."),
    };
    let body = format!("<!doctype html><title>kraken-cli</title><p>{message}</p>\n");
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: \
         {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: \
         close\r\n\r\n{body}",
        body.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    fn state() -> State {
        State::fixed("s3cr3t-state")
    }

    #[test]
    fn a_matching_callback_yields_its_code() {
        let callback = parse(
            "GET /callback?code=abc&state=s3cr3t-state HTTP/1.1",
            &state(),
        );
        let Callback::Code(code) = callback else {
            panic!("{callback:?}")
        };
        assert_eq!(code.expose(), "abc");
    }

    #[test]
    fn a_denied_consent_ends_the_login() {
        let callback = parse(
            "GET /callback?error=access_denied&state=s3cr3t-state HTTP/1.1",
            &state(),
        );
        assert!(matches!(callback, Callback::Denied));
    }

    #[test]
    fn a_wrong_or_missing_state_is_stray_even_when_it_carries_an_error() {
        for line in [
            "GET /callback?code=abc&state=guessed HTTP/1.1",
            "GET /callback?code=abc HTTP/1.1",
            "GET /callback?error=access_denied&state=guessed HTTP/1.1",
        ] {
            assert!(matches!(parse(line, &state()), Callback::Stray), "{line}");
        }
    }

    #[test]
    fn a_repeated_parameter_is_stray() {
        let line = "GET /callback?code=a&code=b&state=s3cr3t-state HTTP/1.1";
        assert!(matches!(parse(line, &state()), Callback::Stray));
    }

    #[test]
    fn other_paths_and_methods_are_not_found() {
        for line in [
            "GET /favicon.ico HTTP/1.1",
            "POST /callback?code=a&state=s3cr3t-state HTTP/1.1",
            "",
        ] {
            assert!(
                matches!(parse(line, &state()), Callback::NotFound),
                "{line:?}"
            );
        }
    }

    #[tokio::test]
    async fn the_response_page_never_echoes_the_code() {
        let (mut browser, mut server) = duplex(16 * 1024);
        browser
            .write_all(b"GET /callback?code=secret-code&state=s3cr3t-state HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let callback = serve(&mut server, &state()).await.unwrap();
        assert!(matches!(callback, Callback::Code(_)));

        let mut page = String::new();
        browser.read_to_string(&mut page).await.unwrap();
        assert!(page.starts_with("HTTP/1.1 200 OK"));
        assert!(page.contains("Cache-Control: no-store"));
        assert!(!page.contains("secret-code"));
    }

    #[tokio::test]
    async fn a_code_survives_a_browser_that_hangs_up_before_the_page() {
        let (mut browser, mut server) = duplex(16 * 1024);
        browser
            .write_all(b"GET /callback?code=the-code&state=s3cr3t-state HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        drop(browser);
        let callback = serve(&mut server, &state()).await.unwrap();
        assert!(matches!(callback, Callback::Code(_)));
    }

    #[tokio::test]
    async fn an_oversized_request_line_is_refused() {
        let (mut browser, mut server) = duplex(64 * 1024);
        let oversized = usize::try_from(MAX_REQUEST_LINE_BYTES).unwrap() + 1024;
        browser.write_all(&vec![b'a'; oversized]).await.unwrap();
        let error = serve(&mut server, &state()).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn large_localhost_cookies_never_drop_the_redirect() {
        let (mut browser, mut server) = duplex(64 * 1024);
        let request = format!(
            "GET /callback?code=the-code&state=s3cr3t-state HTTP/1.1\r\nHost: \
             localhost\r\nCookie: session={}\r\n\r\n",
            "c".repeat(32 * 1024)
        );
        browser.write_all(request.as_bytes()).await.unwrap();
        let callback = serve(&mut server, &state()).await.unwrap();
        assert!(matches!(callback, Callback::Code(_)));
    }

    #[tokio::test]
    async fn a_head_that_never_ends_is_refused_at_its_cap() {
        let (mut browser, mut server) = duplex(64 * 1024);
        let field = format!("X-Filler: {}\r\n", "f".repeat(1024));
        let fields = MAX_HEAD_BYTES / 1024 + 1;
        let writer = tokio::spawn(async move {
            browser
                .write_all(b"GET /callback?code=a&state=s3cr3t-state HTTP/1.1\r\n")
                .await?;
            for _ in 0..fields {
                browser.write_all(field.as_bytes()).await?;
            }
            // Stay open: only the cap, not EOF, may end the read.
            std::future::pending::<io::Result<()>>().await
        });
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            serve(&mut server, &state()),
        )
        .await
        .expect("the head cap must end the read")
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        writer.abort();
    }
}
