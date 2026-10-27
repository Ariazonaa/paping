//! `--tui`: a live full-screen view of one target, with one series per address.

mod digits;
mod render;

use std::collections::VecDeque;
use std::io;
use std::net::IpAddr;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

pub use render::render;

use crate::cli::Config;
use crate::engine::{EngineHandle, Event};
use crate::probe::ProbeResult;
use crate::resolve::Target;
use crate::stats::StatsSet;

/// How many attempts per address the chart and histogram can use.
pub const HISTORY: usize = 1000;
/// How many results the log keeps.
pub const LOG_LINES: usize = 200;

const TICK: Duration = Duration::from_millis(50);

/// One row of the log.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// `"v4"` or `"v6"`.
    pub family: &'static str,
    pub result: ProbeResult,
}

pub struct TuiState {
    pub target: Target,
    pub port: u16,
    pub tls: bool,
    pub http: bool,
    pub started: Instant,
    /// Per address (in `target.addrs` order): connect times in ms; `None` = failed.
    pub history: Vec<VecDeque<Option<f64>>>,
    pub log: VecDeque<LogEntry>,
    /// Per address: the latest result.
    pub last: Vec<Option<ProbeResult>>,
    pub paused: bool,
    /// All `-c` attempts are done; the view stays until `q`.
    pub finished: bool,
    pub stats: StatsSet,
}

impl TuiState {
    pub fn new(target: Target, cfg: &Config) -> Self {
        let n = target.addrs.len();
        let stats = StatsSet::new(&target.addrs);
        TuiState {
            target,
            port: cfg.port,
            tls: cfg.tls,
            http: cfg.http,
            started: Instant::now(),
            history: vec![VecDeque::new(); n],
            log: VecDeque::new(),
            last: vec![None; n],
            paused: false,
            finished: false,
            stats,
        }
    }

    pub fn apply(&mut self, r: ProbeResult) {
        let index = self.index_of(r.addr);
        self.stats.record(&r);
        let connect_ms = r
            .connect
            .filter(|_| r.ok())
            .map(|d| d.as_secs_f64() * 1000.0);
        push_capped(&mut self.history[index], connect_ms, HISTORY);
        let entry = LogEntry {
            family: family(r.addr),
            result: r.clone(),
        };
        push_capped(&mut self.log, entry, LOG_LINES);
        self.last[index] = Some(r);
    }

    fn index_of(&self, addr: IpAddr) -> usize {
        self.target
            .addrs
            .iter()
            .position(|a| *a == addr)
            .unwrap_or(0)
    }
}

pub(crate) fn family(addr: IpAddr) -> &'static str {
    if addr.is_ipv4() { "v4" } else { "v6" }
}

fn push_capped<T>(queue: &mut VecDeque<T>, value: T, cap: usize) {
    if queue.len() == cap {
        queue.pop_front();
    }
    queue.push_back(value);
}

/// Runs the view until `q`, `Esc` or Ctrl+C and returns the statistics. The terminal is
/// restored on the way out, also when drawing fails; `ratatui::init` also installs a
/// panic hook that restores it.
pub fn run(
    cfg: &Config,
    target: Target,
    rx: Receiver<Event>,
    engines: &[EngineHandle],
) -> io::Result<StatsSet> {
    let mut terminal = ratatui::init();
    let mut state = TuiState::new(target, cfg);
    let result = event_loop(&mut terminal, &mut state, &rx, engines);
    ratatui::restore();
    result.map(|()| state.stats)
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    state: &mut TuiState,
    rx: &Receiver<Event>,
    engines: &[EngineHandle],
) -> io::Result<()> {
    let mut running = engines.len();
    loop {
        loop {
            match rx.try_recv() {
                Ok(Event::Probe(r)) => state.apply(r),
                Ok(Event::Done) => {
                    running = running.saturating_sub(1);
                    state.finished = running == 0;
                }
                Ok(Event::Interrupted) => return Ok(()),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        terminal.draw(|frame| render(frame, state))?;
        if !event::poll(TICK)? {
            continue;
        }
        let TermEvent::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(());
            }
            KeyCode::Char('p') => {
                state.paused = !state.paused;
                for engine in engines {
                    engine.set_paused(state.paused);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::cli::{Family, OutputMode};
    use crate::probe::ProbeError;
    use jiff::Timestamp;

    pub(crate) fn cfg() -> Config {
        Config {
            destination: "example.com".into(),
            port: 443,
            timeout: Duration::from_millis(1000),
            count: None,
            color: true,
            bind: None,
            family: Family::Any,
            interval: Duration::from_millis(1000),
            tls: false,
            details: false,
            output: OutputMode::Tui,
            http: false,
            http_path: "/".into(),
        }
    }

    pub(crate) fn v4() -> IpAddr {
        "192.0.2.10".parse().unwrap()
    }

    pub(crate) fn v6() -> IpAddr {
        "2001:db8::5".parse().unwrap()
    }

    pub(crate) fn state(addrs: Vec<IpAddr>) -> TuiState {
        TuiState::new(
            Target {
                host: "example.com".into(),
                addrs,
                is_literal: false,
                dns_time: Some(Duration::from_micros(12_340)),
            },
            &cfg(),
        )
    }

    pub(crate) fn ok(addr: IpAddr, seq: u64, ms: f64) -> ProbeResult {
        ProbeResult {
            seq,
            addr,
            at: Timestamp::UNIX_EPOCH,
            connect: Some(Duration::from_secs_f64(ms / 1000.0)),
            tls: None,
            http_status: None,
            ttfb: None,
            error: None,
        }
    }

    pub(crate) fn timeout(addr: IpAddr, seq: u64) -> ProbeResult {
        ProbeResult {
            connect: None,
            error: Some(ProbeError::Timeout),
            ..ok(addr, seq, 0.0)
        }
    }

    #[test]
    fn history_and_log_are_capped() {
        let mut s = state(vec![v4()]);
        for seq in 1..=1500 {
            s.apply(ok(v4(), seq, 1.0));
        }
        assert_eq!(s.history[0].len(), HISTORY);
        assert_eq!(s.log.len(), LOG_LINES);
        assert_eq!(s.stats.attempted(), 1500);
        assert_eq!(s.last[0].as_ref().map(|r| r.seq), Some(1500));
    }

    #[test]
    fn results_go_to_their_address() {
        let mut s = state(vec![v4(), v6()]);
        s.apply(ok(v4(), 1, 5.0));
        s.apply(timeout(v6(), 1));
        assert_eq!(s.history[0], [Some(5.0)]);
        assert_eq!(s.history[1], [None]);
        assert_eq!(s.log.back().unwrap().family, "v6");
        assert_eq!(s.stats.get(v6()).unwrap().failed(), 1);
        assert!(s.last[1].as_ref().is_some_and(|r| !r.ok()));
    }
}
