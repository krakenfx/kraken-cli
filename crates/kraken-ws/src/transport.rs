//! The transport seam: a split read/write socket plus the live `tokio-tungstenite`
//! implementation. Split so the actor can hold the [`ConnWriter`] while `select!`-ing
//! on the [`ConnReader`] (one combined `&mut` can't), and so a mock can script a feed.
//! Every write is time-bounded here; the read's liveness bound stays with the callers —
//! an inner timer would reset on every `select!` re-poll and never trip.

use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use kraken_core::error::{Error, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as RustlsError, SignatureScheme};
use serde::Serialize;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, Request};
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};
use tokio_tungstenite::{
    Connector as TlsConnector, MaybeTlsStream, WebSocketStream, connect_async,
    connect_async_tls_with_config,
};

const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// A connected `tokio-tungstenite` stream.
type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// An event read from a connection. This seam ships raw frames — parsing them, and
/// tolerating ones that fail the pinned schema, is each consumer's concern.
#[derive(Debug)]
pub(crate) enum ConnEvent {
    /// One raw inbound text frame, verbatim — a cheaply-cloneable view of the wire
    /// bytes, not a copy.
    Frame(Utf8Bytes),
    /// The peer closed, or the stream ended.
    Closed,
    Error(Error),
}

/// The write half of a live connection.
pub(crate) trait ConnWriter: Send {
    /// Serialize `frame` and send it as one text frame.
    async fn send_frame(&mut self, frame: &impl Serialize) -> Result<()>;

    /// Close handshake. The outcome is the caller's to interpret — teardown paths trace a
    /// failure and drop the writer (an abrupt TCP close) rather than propagate it.
    async fn close(&mut self) -> Result<()>;
}

/// The read half of a live connection.
pub(crate) trait ConnReader: Send {
    /// Await the next meaningful event, skipping ping/pong/binary frames.
    async fn recv(&mut self) -> ConnEvent;
}

/// Opens split [`ConnWriter`]/[`ConnReader`] pairs on demand, once per (re)connect.
pub(crate) trait Transport: Send + Sync + 'static {
    type Writer: ConnWriter;
    type Reader: ConnReader;
    /// Dial and return a fresh split writer/reader pair.
    async fn connect(&self) -> Result<(Self::Writer, Self::Reader)>;
}

/// Production transport backed by `tokio-tungstenite`. Owns its URL and handshake
/// headers so it stays `'static` for the spawned connection task.
pub(crate) struct TungsteniteTransport {
    url: String,
    skip_verify: bool,
    /// Extra handshake headers (client identity/telemetry), injected by the application —
    /// this crate carries no telemetry knowledge of its own.
    headers: Vec<(String, String)>,
}

impl TungsteniteTransport {
    /// `skip_verify` disables TLS verification (`--accept-invalid-certs`), for beta/UAT hosts only.
    pub(crate) fn new(
        url: impl Into<String>,
        skip_verify: bool,
        headers: Vec<(String, String)>,
    ) -> Self {
        Self {
            url: url.into(),
            skip_verify,
            headers,
        }
    }
}

impl Transport for TungsteniteTransport {
    type Writer = TungWriter;
    type Reader = TungReader;

    async fn connect(&self) -> Result<(TungWriter, TungReader)> {
        let request = handshake_request(&self.url, &self.headers)?;
        let handshake = async {
            if self.skip_verify {
                let tls = dangerous_tls_connector();
                connect_async_tls_with_config(request, None, false, Some(tls)).await
            } else {
                connect_async(request).await
            }
        };
        let (ws, _) = match tokio::time::timeout(CONNECT_TIMEOUT, handshake).await {
            Ok(handshook) => handshook.map_err(Error::transport)?,
            Err(_elapsed) => return Err(Error::ConnectTimeout(CONNECT_TIMEOUT)),
        };
        let (write, read) = ws.split();
        Ok((TungWriter(write), TungReader(read)))
    }
}

/// The write half of a connected `tokio-tungstenite` stream.
pub(crate) struct TungWriter(SplitSink<WsStream, Message>);

impl ConnWriter for TungWriter {
    async fn send_frame(&mut self, frame: &impl Serialize) -> Result<()> {
        let text = serde_json::to_string(frame)?;

        match tokio::time::timeout(SEND_TIMEOUT, self.0.send(Message::Text(text.into()))).await {
            Ok(sent) => sent.map_err(Error::transport),
            Err(_elapsed) => Err(Error::SendTimeout(SEND_TIMEOUT)),
        }
    }

    async fn close(&mut self) -> Result<()> {
        match tokio::time::timeout(CLOSE_TIMEOUT, self.0.close()).await {
            Ok(closed) => closed.map_err(Error::transport),
            Err(_elapsed) => Err(Error::CloseTimeout(CLOSE_TIMEOUT)),
        }
    }
}

/// The read half of a connected `tokio-tungstenite` stream.
pub(crate) struct TungReader(SplitStream<WsStream>);

impl ConnReader for TungReader {
    async fn recv(&mut self) -> ConnEvent {
        loop {
            match self.0.next().await {
                Some(Ok(Message::Text(text))) => return ConnEvent::Frame(text),
                Some(Ok(Message::Close(_))) | None => return ConnEvent::Closed,
                Some(Err(e)) => return ConnEvent::Error(Error::transport(e)),
                // Ping/Pong/Binary: tungstenite queues an auto-Pong on Ping and flushes it
                // on its next read — which this loop performs immediately — so the read half
                // answers Pings itself; Pong/Binary carry nothing in the v2 protocol.
                Some(Ok(_)) => continue,
            }
        }
    }
}

/// Build the WebSocket handshake request for `url`, carrying the caller-injected
/// client/telemetry headers.
fn handshake_request(url: &str, extra: &[(String, String)]) -> Result<Request<()>> {
    let mut request = url.into_client_request().map_err(Error::transport)?;
    let headers = request.headers_mut();
    for (name, value) in extra {
        match (
            name.parse::<tokio_tungstenite::tungstenite::http::HeaderName>(),
            HeaderValue::from_str(value),
        ) {
            (Ok(name), Ok(value)) => {
                headers.insert(name, value);
            }
            // Skip a telemetry header with an unencodable name/value rather than fail the
            // handshake, but surface it so it can be fixed.
            _ => tracing::warn!(header = %name, "skipping telemetry header with invalid value"),
        }
    }
    Ok(request)
}

/// A TLS connector that accepts any server certificate — the one implementation of the
/// `--accept-invalid-certs` escape hatch, shared by the spot and futures WS stacks so
/// the dangerous block exists exactly once. Warns once per process: a reconnect loop
/// would otherwise flood stderr.
pub fn dangerous_tls_connector() -> TlsConnector {
    static TLS_WARN: std::sync::Once = std::sync::Once::new();
    TLS_WARN.call_once(|| {
        tracing::warn!("TLS certificate verification is disabled for WebSocket connections");
    });

    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerify))
        .with_no_client_auth();
    TlsConnector::Rustls(Arc::new(config))
}

/// A `rustls` verifier that accepts any certificate. Never constructed directly:
/// [`dangerous_tls_connector`] is the sole gateway.
#[derive(Debug)]
struct NoVerify;

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, RustlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
            SignatureScheme::ED448,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_carries_injected_headers_and_skips_invalid_ones() {
        let headers = vec![
            ("x-korigin".to_string(), "u004".to_string()),
            ("X-Bad".to_string(), "line\nbreak".to_string()),
        ];
        let req = handshake_request("wss://ws.kraken.com/v2", &headers).unwrap();
        assert_eq!(req.headers().get("x-korigin").unwrap(), "u004");
        assert!(req.headers().get("X-Bad").is_none());
    }
}
