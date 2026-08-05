//! The scripted fake [`Transport`] shared by the connection test suites: records
//! every frame written, feeds each connection's reader from a test-controlled
//! channel, and fails or hangs dials on demand.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kraken_core::error::{Error, Result};
use tokio::sync::mpsc;

use super::transport::{ConnEvent, ConnReader, ConnWriter, Transport};

/// Poll `cond` until true, panicking if it never holds — so an exhausted wait fails
/// loud here instead of falling through to a confusing downstream assert.
pub(crate) async fn wait_until(mut cond: impl FnMut() -> bool) {
    for _ in 0..500 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!("condition not met in time");
}

/// Records frames written across every connection, and exposes a per-connection
/// reader-feed sender so a test can push inbound frames (or close the socket).
#[derive(Clone)]
pub(crate) struct MockTransport {
    sent: Arc<Mutex<Vec<String>>>,
    readers: Arc<Mutex<Vec<mpsc::UnboundedSender<ConnEvent>>>>,
    connects: Arc<AtomicUsize>,
    closes: Arc<AtomicUsize>,
    /// When set, every `connect()` fails — a permanently dead endpoint.
    pub(crate) fail: bool,
    /// Fail this many connect attempts before succeeding — a transient dial failure.
    pub(crate) fail_first: Arc<AtomicUsize>,
    /// When set, `connect()` registers the attempt then never resolves — a hung
    /// handshake, so a test can prove shutdown (or a dial budget) still ends it.
    pub(crate) hang: bool,
}

impl MockTransport {
    pub(crate) fn new() -> Self {
        Self {
            sent: Arc::new(Mutex::new(Vec::new())),
            readers: Arc::new(Mutex::new(Vec::new())),
            connects: Arc::new(AtomicUsize::new(0)),
            closes: Arc::new(AtomicUsize::new(0)),
            fail: false,
            fail_first: Arc::new(AtomicUsize::new(0)),
            hang: false,
        }
    }

    pub(crate) fn sent(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }

    pub(crate) fn connect_count(&self) -> usize {
        self.connects.load(Ordering::SeqCst)
    }

    pub(crate) fn close_count(&self) -> usize {
        self.closes.load(Ordering::SeqCst)
    }

    /// The feed sender for the `nth` connection (0-based), once it has connected.
    pub(crate) fn reader(&self, nth: usize) -> mpsc::UnboundedSender<ConnEvent> {
        self.readers.lock().unwrap()[nth].clone()
    }
}

impl Transport for MockTransport {
    type Writer = MockWriter;
    type Reader = MockReader;

    async fn connect(&self) -> Result<(MockWriter, MockReader)> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(Error::Transport("mock connect failure".into()));
        }
        if self
            .fail_first
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(Error::Transport("mock transient connect failure".into()));
        }
        if self.hang {
            std::future::pending::<()>().await;
        }
        let (tx, rx) = mpsc::unbounded_channel();
        self.readers.lock().unwrap().push(tx);
        Ok((
            MockWriter {
                sent: self.sent.clone(),
                closes: self.closes.clone(),
            },
            MockReader { rx },
        ))
    }
}

pub(crate) struct MockWriter {
    sent: Arc<Mutex<Vec<String>>>,
    closes: Arc<AtomicUsize>,
}

impl ConnWriter for MockWriter {
    async fn send_frame(&mut self, frame: &impl serde::Serialize) -> Result<()> {
        self.sent
            .lock()
            .unwrap()
            .push(serde_json::to_string(frame)?);
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

pub(crate) struct MockReader {
    rx: mpsc::UnboundedReceiver<ConnEvent>,
}

impl ConnReader for MockReader {
    async fn recv(&mut self) -> ConnEvent {
        self.rx.recv().await.unwrap_or(ConnEvent::Closed)
    }
}
