//! A scripted venue for proving what a client resends when a reply is lost.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpStream,
};

/// A captured reply from `fixtures/<venue>/`, as a live 200 would arrive.
pub(crate) fn fixture_reply(venue: &str, path: &str) -> crate::transport::Reply {
    let body = std::fs::read(format!(
        "{}/fixtures/{venue}/{path}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap_or_else(|e| panic!("fixture {venue}/{path}: {e}"));
    crate::transport::Reply {
        status: reqwest::StatusCode::OK,
        body,
    }
}

/// How the venue treats its `n`th request (from 0).
#[derive(Clone, Copy)]
pub(crate) enum Answer {
    /// Holds the socket open, so the client's timeout ends the call.
    Silence,
    /// Sends the headers and part of the body, then hangs up.
    CutOff,
    /// A complete `200` with this JSON body.
    Reply(&'static str),
}

/// Starts a venue that answers its `n`th request with `script(n)`; returns its
/// base URL and how many requests reached it.
pub(crate) async fn scripted_venue(script: fn(usize) -> Answer) -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let received = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&received);
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let counter = Arc::clone(&counter);
            tokio::spawn(async move {
                if !read_request(&mut socket).await {
                    return;
                }
                let n = counter.fetch_add(1, Ordering::SeqCst);
                match script(n) {
                    Answer::Silence => std::future::pending::<()>().await,
                    Answer::CutOff => {
                        let head = "HTTP/1.1 200 OK\r\nContent-Length: 64\r\n\r\n{\"error\":";
                        let _ = socket.write_all(head.as_bytes()).await;
                    }
                    Answer::Reply(body) => {
                        let reply = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: \
                             {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = socket.write_all(reply.as_bytes()).await;
                    }
                }
            });
        }
    });
    (base_url, received)
}

pub(crate) async fn silent_venue() -> (String, Arc<AtomicUsize>) {
    scripted_venue(|_| Answer::Silence).await
}

/// Reads one request, head and body, so closing the socket never resets a
/// connection with unread bytes. `false` when the client hung up first.
async fn read_request(socket: &mut TcpStream) -> bool {
    let mut request = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if request.len() >= end + 4 + length {
                return true;
            }
        }
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => return false,
            Ok(n) => request.extend_from_slice(&chunk[..n]),
        }
    }
}
