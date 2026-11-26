//! One tracing setup for every binary.
//!
//! Filter with `VICTORY_LOG` (falls back to `RUST_LOG`), e.g.
//! `VICTORY_LOG=victory_loader=trace,info`. Debug and trace output exist
//! only in builds with the `diagnostics` feature; a plain release build
//! logs info and above whatever the filter asks for.

use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// The most verbose level this build can log.
const CAP: LevelFilter = if cfg!(feature = "diagnostics") {
    LevelFilter::TRACE
} else {
    LevelFilter::INFO
};

pub fn init(default_level: &str) {
    let filter = std::env::var("VICTORY_LOG")
        .or_else(|_| std::env::var("RUST_LOG"))
        .ok()
        .and_then(|v| EnvFilter::try_new(v).ok())
        .unwrap_or_else(|| EnvFilter::new(default_level));
    let sync = std::env::var_os("VICTORY_LOG_SYNC").is_some_and(|v| v == "1");
    let registry = tracing_subscriber::registry().with(CAP).with(filter);
    let _ = if sync {
        registry.with(tracing_subscriber::fmt::layer().with_target(true).with_writer(std::io::stderr)).try_init()
    } else {
        start_writer();
        registry.with(tracing_subscriber::fmt::layer().with_target(true).with_writer(|| Line(Vec::new()))).try_init()
    };
}

/// Lines waiting for the writer thread ([`start_writer`]); a logging thread
/// never waits for the file (`VICTORY_LOG_SYNC=1`: each thread writes to
/// stderr itself, under its lock, as before). Unbuffered stderr writes under
/// that one lock held the submitting thread 2.7-3.8 s at a time in a run whose
/// frames took 4.9 s (run twentytwo's timeline).
static LINES: std::sync::OnceLock<std::sync::mpsc::SyncSender<Vec<u8>>> = std::sync::OnceLock::new();

/// Lines dropped because the writer was behind by the whole queue.
static DROPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// One formatted event, handed to the writer when dropped.
struct Line(Vec<u8>);

impl std::io::Write for Line {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for Line {
    fn drop(&mut self) {
        use std::io::Write;
        use std::sync::mpsc::TrySendError;
        if self.0.is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut self.0);
        match LINES.get().map(|lines| lines.try_send(bytes)) {
            Some(Ok(())) => {}
            Some(Err(TrySendError::Full(_))) => {
                DROPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            Some(Err(TrySendError::Disconnected(bytes))) => {
                let _ = std::io::stderr().write_all(&bytes);
            }
            None => {}
        }
    }
}

/// The thread that writes the queued lines to stderr, through a buffer
/// flushed when the queue runs dry and at least every 200 ms.
fn start_writer() {
    use std::io::Write;
    let (to, from) = std::sync::mpsc::sync_channel::<Vec<u8>>(1 << 16);
    if LINES.set(to).is_err() {
        return;
    }
    let _ = std::thread::Builder::new().name("victory-log".into()).spawn(move || {
        use std::sync::mpsc::RecvTimeoutError;
        let mut out = std::io::BufWriter::with_capacity(1 << 20, std::io::stderr());
        loop {
            match from.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(bytes) => {
                    let _ = out.write_all(&bytes);
                    while let Ok(bytes) = from.try_recv() {
                        let _ = out.write_all(&bytes);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            let dropped = DROPPED.swap(0, std::sync::atomic::Ordering::Relaxed);
            if dropped > 0 {
                let _ = writeln!(out, "victory-log: {dropped} lines dropped (the writer was behind)");
            }
            let _ = out.flush();
        }
    });
}
