use std::io::{self, IsTerminal};
use std::time::Duration;

use jiff::Timestamp;

use crate::cli::Config;
use crate::probe::ProbeResult;
use crate::resolve::Target;
use crate::stats::StatsSet;

pub mod csv;
pub mod json;
pub mod text;

/// One way of printing a run. `main` feeds it the start, every attempt, and the end.
pub trait Sink {
    fn start(&mut self, target: &Target, cfg: &Config) -> io::Result<()>;
    fn probe(&mut self, r: &ProbeResult, target: &Target, cfg: &Config) -> io::Result<()>;
    fn finish(&mut self, stats: &StatsSet, cfg: &Config) -> io::Result<()>;
}

/// Whether the text output may redraw a status line and show the trend: stdout is a
/// terminal that understands VT sequences. Independent of `--nocolor`.
pub fn use_interactive() -> bool {
    let is_terminal = io::stdout().is_terminal();
    let term = std::env::var("TERM").ok();
    #[cfg(windows)]
    let vt = is_terminal && crossterm::ansi_support::supports_ansi();
    #[cfg(not(windows))]
    let vt = true;
    allow_interactive(is_terminal, term.as_deref(), vt)
}

fn allow_interactive(is_terminal: bool, term: Option<&str>, vt: bool) -> bool {
    is_terminal && vt && term != Some("dumb")
}

/// Colors stay on only when `--nocolor` was not given, stdout is a terminal, and
/// `NO_COLOR` is unset or empty. On Windows this also switches the console to VT mode.
pub fn use_color(flag: bool) -> bool {
    if !flag || !io::stdout().is_terminal() {
        return false;
    }
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    #[cfg(windows)]
    {
        crossterm::ansi_support::supports_ansi()
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// Milliseconds rounded to microseconds, for JSON.
fn ms_rounded(d: Duration) -> f64 {
    round_to(d.as_secs_f64() * 1000.0, 3)
}

fn round_to(value: f64, decimals: i32) -> f64 {
    let factor = 10f64.powi(decimals);
    (value * factor).round() / factor
}

/// RFC 3339 in UTC with milliseconds, for JSON and CSV.
fn timestamp(at: Timestamp) -> String {
    at.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_needs_a_vt_terminal() {
        assert!(allow_interactive(true, Some("xterm-256color"), true));
        assert!(allow_interactive(true, None, true));
        assert!(!allow_interactive(false, Some("xterm"), true), "pipe");
        assert!(!allow_interactive(true, Some("dumb"), true), "TERM=dumb");
        assert!(!allow_interactive(true, None, false), "console without VT");
    }
}
