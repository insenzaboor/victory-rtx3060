//! A thread of its own that drops what it is handed, with a cap on what may
//! wait for it.
//!
//! For objects whose dropping may come later than where they are let go
//! (`crate::host`'s retired objects: they only wait for the card), so that
//! the thread letting them go does not pay for their releases (D3D12
//! `Release`, heap frees). What waits for the thread is not freed: past the
//! cap a batch is dropped by the caller, as without the thread, and that is
//! said once.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;

/// See the module.
pub(crate) struct Dropper<T: Send + 'static> {
    /// `None`: the thread could not be started, everything is dropped by
    /// the caller.
    send: Option<Sender<Vec<T>>>,
    /// Items handed to the thread and not dropped yet.
    pending: Arc<AtomicUsize>,
    /// More waiting than this with a batch: the batch is dropped by the caller.
    most: usize,
    /// For the log line: the switch, and whether it was said.
    switch: &'static str,
    said: AtomicBool,
}

impl<T: Send + 'static> Dropper<T> {
    /// Starts the thread `name`, whose drops show as `span` in the timeline.
    /// `most`: the items that may wait for it.
    pub(crate) fn new(name: &'static str, span: &'static str, most: usize, switch: &'static str) -> Self {
        let pending = Arc::new(AtomicUsize::new(0));
        let waiting = pending.clone();
        let (send, batches) = std::sync::mpsc::channel::<Vec<T>>();
        let started = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                for batch in batches {
                    let n = batch.len();
                    let _dropping = crate::timeline::span(span);
                    drop(batch);
                    waiting.fetch_sub(n, Ordering::AcqRel);
                }
            })
            .is_ok();
        Dropper { send: started.then_some(send), pending, most, switch, said: AtomicBool::new(false) }
    }

    /// Items handed to the thread and not dropped yet.
    #[cfg(test)]
    fn pending(&self) -> usize {
        self.pending.load(Ordering::Acquire)
    }

    /// Hands `batch` to the thread to drop, or drops it here: when the
    /// thread is not there, or with it more than [`Dropper::most`] would
    /// wait. Whether it was handed.
    pub(crate) fn drop_aside(&self, batch: Vec<T>) -> bool {
        let n = batch.len();
        if n == 0 {
            return true;
        }
        let Some(send) = &self.send else {
            drop(batch);
            return false;
        };
        let waiting = self.pending.fetch_add(n, Ordering::AcqRel);
        if waiting + n > self.most {
            self.pending.fetch_sub(n, Ordering::AcqRel);
            if !self.said.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    waiting,
                    batch = n,
                    most = self.most,
                    "{}: more than the most would wait for the thread that drops them: dropped where they were let go, as without the switch (said once)",
                    self.switch
                );
            }
            drop(batch);
            return false;
        }
        match send.send(batch) {
            Ok(()) => true,
            Err(std::sync::mpsc::SendError(batch)) => {
                // (The thread is gone: dropped here.)
                self.pending.fetch_sub(n, Ordering::AcqRel);
                drop(batch);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Dropper;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Counts its drops and notes the thread that dropped it.
    struct Counted {
        dropped: Arc<AtomicUsize>,
        on: Arc<Mutex<Vec<std::thread::ThreadId>>>,
        /// If there: waits for it before being dropped.
        gate: Option<std::sync::mpsc::Receiver<()>>,
    }

    impl Drop for Counted {
        fn drop(&mut self) {
            if let Some(gate) = self.gate.take() {
                let _ = gate.recv();
            }
            self.on.lock().unwrap().push(std::thread::current().id());
            self.dropped.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn counters() -> (Arc<AtomicUsize>, Arc<Mutex<Vec<std::thread::ThreadId>>>) {
        (Arc::new(AtomicUsize::new(0)), Arc::new(Mutex::new(Vec::new())))
    }

    fn batch(n: usize, dropped: &Arc<AtomicUsize>, on: &Arc<Mutex<Vec<std::thread::ThreadId>>>) -> Vec<Counted> {
        (0..n).map(|_| Counted { dropped: dropped.clone(), on: on.clone(), gate: None }).collect()
    }

    fn wait_for(what: impl Fn() -> bool) {
        let began = std::time::Instant::now();
        while !what() {
            assert!(began.elapsed() < std::time::Duration::from_secs(10), "the dropping thread did not finish");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Everything handed over is dropped, on the thread of its own.
    #[test]
    fn the_thread_drops_everything() {
        let dropper = Dropper::<Counted>::new("victory-dropper-test", "test: dropping", 1 << 20, "TEST");
        let (dropped, on) = counters();
        for _ in 0..100 {
            assert!(dropper.drop_aside(batch(10, &dropped, &on)));
        }
        wait_for(|| dropped.load(Ordering::Acquire) == 1000);
        wait_for(|| dropper.pending() == 0);
        let me = std::thread::current().id();
        assert!(on.lock().unwrap().iter().all(|t| *t != me));
    }

    /// Past the cap the caller drops the batch, at once, and after the
    /// thread caught up it takes batches again.
    #[test]
    fn over_the_cap_the_caller_drops() {
        let dropper = Dropper::<Counted>::new("victory-dropper-test", "test: dropping", 4, "TEST");
        let (dropped, on) = counters();
        // One that holds the thread up until let through.
        let (open, gate) = std::sync::mpsc::channel();
        assert!(dropper.drop_aside(vec![Counted { dropped: dropped.clone(), on: on.clone(), gate: Some(gate) }]));
        // Up to the cap: handed over.
        assert!(dropper.drop_aside(batch(3, &dropped, &on)));
        assert_eq!(dropper.pending(), 4);
        // Over it: dropped here, before the call returns.
        let (here, here_on) = counters();
        assert!(!dropper.drop_aside(batch(2, &here, &here_on)));
        assert_eq!(here.load(Ordering::Acquire), 2);
        let me = std::thread::current().id();
        assert!(here_on.lock().unwrap().iter().all(|t| *t == me));
        assert_eq!(dropper.pending(), 4);
        // A single batch bigger than the cap: here too.
        assert!(!dropper.drop_aside(batch(5, &here, &here_on)));
        assert_eq!(here.load(Ordering::Acquire), 7);
        // Let through: the thread catches up and takes more.
        open.send(()).unwrap();
        wait_for(|| dropper.pending() == 0);
        assert_eq!(dropped.load(Ordering::Acquire), 4);
        assert!(dropper.drop_aside(batch(4, &dropped, &on)));
        wait_for(|| dropped.load(Ordering::Acquire) == 8);
        assert!(on.lock().unwrap().iter().all(|t| *t != me));
    }

    /// Nothing to hand over is not a fallback.
    #[test]
    fn an_empty_batch_is_nothing() {
        let dropper = Dropper::<Counted>::new("victory-dropper-test", "test: dropping", 0, "TEST");
        assert!(dropper.drop_aside(Vec::new()));
        assert_eq!(dropper.pending(), 0);
    }
}
