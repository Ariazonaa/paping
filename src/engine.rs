use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::probe::{ProbeResult, ProbeSpec, probe};

#[derive(Debug)]
pub enum Event {
    Probe(ProbeResult),
    /// All `-c` attempts are done.
    Done,
    /// Ctrl+C; sent by the signal handler, not by the engine.
    Interrupted,
}

pub struct EngineHandle {
    paused: Arc<AtomicBool>,
    /// Only read by tests; dropping a `JoinHandle` detaches the thread.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) thread: JoinHandle<()>,
}

impl EngineHandle {
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }
}

/// Starts the probe thread. It is never joined: on Ctrl+C the main thread prints the
/// statistics and exits without waiting for an attempt that may still be in flight.
///
/// Each round probes every spec at the same time and sends the results in spec order
/// once all of them are done. Rounds start `interval` apart (start to start); a round
/// that takes longer than the interval is followed by the next one at once, as in the
/// original. So two address families share one clock and never drift apart.
pub fn spawn(
    specs: Vec<ProbeSpec>,
    count: Option<u64>,
    interval: Duration,
    events: Sender<Event>,
) -> EngineHandle {
    let paused = Arc::new(AtomicBool::new(false));
    let thread = {
        let paused = paused.clone();
        thread::spawn(move || {
            let mut seq = 0;
            loop {
                if count.is_some_and(|count| seq >= count) {
                    let _ = events.send(Event::Done);
                    return;
                }
                if paused.load(Ordering::Relaxed) {
                    thread::sleep(PAUSE_POLL);
                    continue;
                }
                seq += 1;
                let started = Instant::now();
                for result in round(&specs, seq) {
                    if events.send(Event::Probe(result)).is_err() {
                        return; // nobody listens any more
                    }
                }
                if count != Some(seq) {
                    thread::sleep(interval.saturating_sub(started.elapsed()));
                }
            }
        })
    };
    EngineHandle { paused, thread }
}

/// One attempt per spec, all at once; results in spec order.
fn round(specs: &[ProbeSpec], seq: u64) -> Vec<ProbeResult> {
    if let [spec] = specs {
        return vec![probe(spec, seq)];
    }
    thread::scope(|scope| {
        let running: Vec<_> = specs
            .iter()
            .map(|spec| scope.spawn(move || probe(spec, seq)))
            .collect();
        running
            .into_iter()
            .map(|handle| handle.join().expect("a probe does not panic"))
            .collect()
    })
}

const PAUSE_POLL: Duration = Duration::from_millis(50);

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{SocketAddr, TcpListener};
    use std::sync::mpsc::{self, RecvTimeoutError};

    fn listener() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        thread::spawn(move || for _ in l.incoming() {});
        addr
    }

    fn spec() -> ProbeSpec {
        ProbeSpec {
            addr: listener(),
            bind: None,
            timeout: Duration::from_millis(1000),
            tls: None,
            http: None,
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn runs_count_then_done() {
        let (tx, rx) = mpsc::channel();
        let started = Instant::now();
        let _engine = spawn(vec![spec()], Some(3), ms(100), tx);
        for seq in 1..=3 {
            match rx.recv_timeout(ms(2000)) {
                Ok(Event::Probe(r)) => {
                    assert_eq!(r.seq, seq);
                    assert!(r.ok(), "{:?}", r.error);
                }
                other => panic!("expected probe {seq}, got {other:?}"),
            }
        }
        assert!(matches!(rx.recv_timeout(ms(2000)), Ok(Event::Done)));
        assert!(started.elapsed() >= ms(200));
    }

    #[test]
    fn pause_stops_probing() {
        let (tx, rx) = mpsc::channel();
        let engine = spawn(vec![spec()], None, ms(50), tx);
        assert!(matches!(rx.recv_timeout(ms(2000)), Ok(Event::Probe(_))));
        engine.set_paused(true);
        assert!(engine.is_paused());
        // At most one attempt may already be under way when the pause lands.
        thread::sleep(ms(100));
        while rx.try_recv().is_ok() {}
        assert!(matches!(
            rx.recv_timeout(ms(300)),
            Err(RecvTimeoutError::Timeout)
        ));
        engine.set_paused(false);
        assert!(matches!(rx.recv_timeout(ms(2000)), Ok(Event::Probe(_))));
    }

    #[test]
    fn stops_when_receiver_dropped() {
        let (tx, rx) = mpsc::channel();
        let engine = spawn(vec![spec()], None, ms(20), tx);
        assert!(matches!(rx.recv_timeout(ms(2000)), Ok(Event::Probe(_))));
        drop(rx);
        let started = Instant::now();
        while !engine.thread.is_finished() {
            assert!(started.elapsed() < ms(500), "engine thread still running");
            thread::sleep(ms(10));
        }
    }
}
