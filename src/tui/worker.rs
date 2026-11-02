//! Collecting in the background: [`facts::collect`] every `poll` seconds and on demand, on a
//! thread of its own so the board never waits on `claude agents` or a transcript parse. It never
//! touches the terminal (all input is read on the main thread).
//!
//! - One collect at a time. A refresh asked for while one runs sets a "run again" flag, so it
//!   runs once more straight after, never queued up.
//! - Paused while you are in a session: nothing is collected, and a collect that was running
//!   when the pause began is thrown away, so the first snapshot after you come back is fresh.
//!   Resuming collects at once.
//! - A collect that panics becomes an error message; the board keeps its last good snapshot.

use crate::facts::{self, Snapshot, Sources};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The collect thread's name; the panic hook leaves its (caught) panics alone.
pub const THREAD: &str = "collect";

/// What a collect produced: a snapshot, or why there is none.
pub type Collected = Result<Snapshot, String>;

#[derive(Default)]
struct Control {
    /// Collect as soon as the current one (if any) is done.
    again: bool,
    paused: bool,
    quit: bool,
    /// Bumped by every pause; a collect started under an older epoch is stale.
    epoch: u64,
}

struct Shared {
    control: Mutex<Control>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Control> {
        // A panic while holding this lock can't leave it inconsistent (plain flags).
        self.control.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct Worker {
    shared: Arc<Shared>,
    results: Receiver<(u64, Collected)>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    /// Start collecting from `sources` every `poll`, beginning now.
    pub fn start(sources: Sources, poll: Duration) -> Worker {
        Worker::start_with(poll, move |generation| facts::collect(&sources, generation))
    }

    /// [`Worker::start`] with the collect given, for tests.
    pub fn start_with(poll: Duration, collect: impl Fn(u64) -> Snapshot + Send + 'static) -> Worker {
        let shared = Arc::new(Shared { control: Mutex::new(Control { again: true, ..Control::default() }), wake: Condvar::new() });
        let (tx, results) = mpsc::channel();
        let thread = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name(THREAD.into())
                .spawn(move || run(&shared, poll, collect, tx))
                .expect("start the collect thread")
        };
        Worker { shared, results, thread: Some(thread) }
    }

    /// Collect now, or straight after the collect that is running.
    pub fn refresh(&self) {
        self.shared.lock().again = true;
        self.shared.wake.notify_one();
    }

    /// Stop collecting (while a session has the terminal); drop what's in flight.
    pub fn pause(&self) {
        let mut c = self.shared.lock();
        c.paused = true;
        c.epoch += 1;
    }

    /// Collect again, starting now.
    pub fn resume(&self) {
        let mut c = self.shared.lock();
        c.paused = false;
        c.again = true;
        drop(c);
        self.shared.wake.notify_one();
    }

    /// The newest result since the last call, if any; stale ones are skipped.
    pub fn try_recv(&self) -> Option<Collected> {
        let mut newest = None;
        while let Ok((epoch, r)) = self.results.try_recv() {
            if epoch == self.shared.lock().epoch {
                newest = Some(r);
            }
        }
        newest
    }

    /// Wait up to `timeout` for a result (for tests and the first snapshot).
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Collected> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.checked_duration_since(Instant::now())?;
            let (epoch, r) = self.results.recv_timeout(left).ok()?;
            if epoch == self.shared.lock().epoch {
                return Some(r);
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.lock().quit = true;
        self.shared.wake.notify_one();
        // Don't wait for a collect stuck on `claude agents`: it has its own deadline, and the
        // process is exiting anyway.
        drop(self.thread.take());
    }
}

fn run(shared: &Shared, poll: Duration, collect: impl Fn(u64) -> Snapshot, tx: Sender<(u64, Collected)>) {
    let mut generation = 0;
    let mut next = Instant::now();
    loop {
        let epoch = {
            let mut c = shared.lock();
            loop {
                if c.quit {
                    return;
                }
                let due = Instant::now() >= next;
                if !c.paused && (c.again || due) {
                    break;
                }
                let wait = if c.paused { poll } else { next.saturating_duration_since(Instant::now()) };
                c = shared.wake.wait_timeout(c, wait).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
            }
            c.again = false;
            c.epoch
        };
        generation += 1;
        let result = catch_unwind(AssertUnwindSafe(|| collect(generation))).map_err(|panic| {
            let why = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("unknown error");
            format!("Couldn't read the sessions: {why}")
        });
        next = Instant::now() + poll;
        if tx.send((epoch, result)).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn collects_at_once_on_refresh_and_reports_a_panic() {
        let calls = Arc::new(AtomicU64::new(0));
        let n = Arc::clone(&calls);
        let w = Worker::start_with(Duration::from_secs(3600), move |generation| {
            if n.fetch_add(1, Ordering::SeqCst) == 1 {
                panic!("boom");
            }
            Snapshot { generation, ..Snapshot::default() }
        });
        let first = w.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        assert_eq!(first.generation, 1);
        w.refresh();
        let err = w.recv_timeout(Duration::from_secs(5)).unwrap().unwrap_err();
        assert!(err.contains("boom"), "{err}");
        w.refresh();
        assert_eq!(w.recv_timeout(Duration::from_secs(5)).unwrap().unwrap().generation, 3);
    }

    #[test]
    fn nothing_is_collected_while_paused() {
        let w = Worker::start_with(Duration::from_millis(20), |generation| Snapshot { generation, ..Snapshot::default() });
        assert!(w.recv_timeout(Duration::from_secs(5)).is_some());
        w.pause();
        w.refresh();
        std::thread::sleep(Duration::from_millis(100));
        assert!(w.try_recv().is_none());
        w.resume();
        assert!(w.recv_timeout(Duration::from_secs(5)).is_some());
    }
}
