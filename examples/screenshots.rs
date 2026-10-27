//! Renders the README images from paping's real output code:
//!
//!     cargo run --example screenshots
//!
//! writes `docs/images/terminal.svg` (colored text mode with the live status line) and
//! `docs/images/tui.svg` (`--tui`). The data is fixed sample data for one.one.one.one, so
//! the images are reproducible and need no network.

use std::fmt::Write as _;
use std::fs;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use jiff::Timestamp;
use paping::cli::{Config, Family, OutputMode};
use paping::output::text::{header, probe_line, status_line};
use paping::probe::{ProbeError, ProbeResult};
use paping::resolve::Target;
use paping::stats::StatsSet;
use paping::tui::{TuiState, render};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};

const CELL_W: f64 = 8.4;
const CELL_H: f64 = 18.0;
const FONT_SIZE: f64 = 14.0;
const PAD: f64 = 16.0;
const TITLE_BAR: f64 = 34.0;

const BG: &str = "#0d1117";
const FG: &str = "#c9d1d9";
const DIM: &str = "#6e7681";

fn v4() -> IpAddr {
    "1.1.1.1".parse().unwrap()
}

fn v6() -> IpAddr {
    "2606:4700:4700::1111".parse().unwrap()
}

fn target() -> Target {
    Target {
        host: "one.one.one.one".into(),
        addrs: vec![v4(), v6()],
        is_literal: false,
        dns_time: Some(Duration::from_micros(11_870)),
    }
}

fn config() -> Config {
    Config {
        destination: "one.one.one.one".into(),
        port: 443,
        timeout: Duration::from_millis(1000),
        count: None,
        color: true,
        bind: None,
        family: Family::Any,
        interval: Duration::from_millis(1000),
        tls: true,
        details: false,
        output: OutputMode::Text,
        http: true,
        http_path: "/".into(),
    }
}

fn ms(value: f64) -> Duration {
    Duration::from_secs_f64(value / 1000.0)
}

/// A plausible attempt: connect, TLS and TTFB that move a little from round to round.
fn sample(addr: IpAddr, seq: u64) -> ProbeResult {
    let wobble = (seq as f64 * 0.9).sin() * 0.6;
    let base = if addr.is_ipv4() { 9.1 } else { 9.8 } + wobble;
    let spike = if seq.is_multiple_of(23) { 7.5 } else { 0.0 };
    let failed = (addr.is_ipv6() && (seq == 3 || seq % 29 == 8)) || seq == 47;
    ProbeResult {
        seq,
        addr,
        at: Timestamp::UNIX_EPOCH,
        connect: (!failed).then(|| ms(base + spike)),
        tls: (!failed).then(|| ms(base + 2.3)),
        http_status: (!failed).then_some(200),
        ttfb: (!failed).then(|| ms(base * 2.1 + 3.0)),
        error: failed.then_some(ProbeError::Timeout),
    }
}

/// One styled piece of a row, starting at column `col`.
struct Run {
    col: usize,
    text: String,
    fg: String,
    bg: Option<String>,
    bold: bool,
}

fn ansi_color(code: u8) -> &'static str {
    match code {
        31 => "#f85149",
        32 => "#3fb950",
        33 => "#d29922",
        34 => "#58a6ff",
        _ => FG,
    }
}

/// Splits a line with paping's `\x1b[1;NNm … \x1b[0m` colors into runs.
fn parse_ansi(line: &str, default_fg: &str) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut col = 0;
    let mut fg = default_fg.to_string();
    let mut bold = false;
    let mut rest = line;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("\x1b[") {
            let end = after.find('m').expect("SGR ends with m");
            let params = &after[..end];
            if params == "0" {
                fg = default_fg.to_string();
                bold = false;
            } else {
                for p in params.split(';') {
                    match p.parse::<u8>().unwrap_or(0) {
                        1 => bold = true,
                        code @ 30..=37 => fg = ansi_color(code).to_string(),
                        _ => {}
                    }
                }
            }
            rest = &after[end + 1..];
            continue;
        }
        let next = rest.find('\x1b').unwrap_or(rest.len());
        let text = &rest[..next];
        runs.push(Run {
            col,
            text: text.to_string(),
            fg: fg.clone(),
            bg: None,
            bold,
        });
        col += text.chars().count();
        rest = &rest[next..];
    }
    runs
}

fn ratatui_color(color: Color) -> Option<&'static str> {
    Some(match color {
        Color::Reset => return None,
        Color::Black => BG,
        Color::Red | Color::LightRed => "#f85149",
        Color::Green | Color::LightGreen => "#3fb950",
        Color::Yellow | Color::LightYellow => "#d29922",
        Color::Blue | Color::LightBlue => "#58a6ff",
        Color::Cyan | Color::LightCyan => "#39c5cf",
        Color::DarkGray => DIM,
        _ => FG,
    })
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A terminal window with `rows` (each a list of runs) on a dark background.
fn svg(title: &str, cols: usize, rows: &[Vec<Run>]) -> String {
    let width = PAD * 2.0 + cols as f64 * CELL_W;
    let height = TITLE_BAR + PAD * 2.0 + rows.len() as f64 * CELL_H;
    let mut out = String::new();
    writeln!(
        out,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0}" height="{height:.0}" viewBox="0 0 {width:.0} {height:.0}">"##
    )
    .unwrap();
    writeln!(
        out,
        r##"<rect width="100%" height="100%" rx="10" fill="{BG}" stroke="#30363d"/>"##
    )
    .unwrap();
    for (i, color) in ["#ff5f56", "#ffbd2e", "#27c93f"].iter().enumerate() {
        writeln!(
            out,
            r##"<circle cx="{}" cy="17" r="6" fill="{color}"/>"##,
            20 + i * 20
        )
        .unwrap();
    }
    writeln!(
        out,
        r##"<text x="{:.0}" y="22" fill="{DIM}" font-family="ui-sans-serif, -apple-system, Segoe UI, sans-serif" font-size="13" text-anchor="middle">{}</text>"##,
        width / 2.0,
        escape(title)
    )
    .unwrap();
    writeln!(
        out,
        r##"<g font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, 'DejaVu Sans Mono', monospace" font-size="{FONT_SIZE}" xml:space="preserve">"##
    )
    .unwrap();
    for (row, runs) in rows.iter().enumerate() {
        let top = TITLE_BAR + PAD + row as f64 * CELL_H;
        for run in runs {
            let len = run.text.chars().count();
            let x = PAD + run.col as f64 * CELL_W;
            if let Some(bg) = &run.bg {
                writeln!(
                    out,
                    r##"<rect x="{x:.1}" y="{top:.1}" width="{:.1}" height="{CELL_H}" fill="{bg}"/>"##,
                    len as f64 * CELL_W
                )
                .unwrap();
            }
            // Every word sits at its own column, so the result does not depend on how a
            // renderer treats spaces or on the font's real glyph widths.
            let weight = if run.bold {
                r#" font-weight="bold""#
            } else {
                ""
            };
            let chars: Vec<char> = run.text.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                if chars[i] == ' ' {
                    i += 1;
                    continue;
                }
                let start = i;
                while i < chars.len() && chars[i] != ' ' {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                writeln!(
                    out,
                    r##"<text x="{:.1}" y="{:.1}" fill="{}"{weight} textLength="{:.1}" lengthAdjust="spacingAndGlyphs">{}</text>"##,
                    PAD + (run.col + start) as f64 * CELL_W,
                    top + CELL_H * 0.76,
                    run.fg,
                    (i - start) as f64 * CELL_W,
                    escape(&word)
                )
                .unwrap();
            }
        }
    }
    out.push_str("</g>\n</svg>\n");
    out
}

/// Text mode on a terminal: the results so far and the live status line under them.
fn terminal_image() -> String {
    let target = target();
    let cfg = config();
    let mut lines = vec![
        "\x1b[1;32m$\x1b[0m paping one.one.one.one -p https --http".to_string(),
        paping::cli::banner(),
        String::new(),
        header(&target, cfg.port, true),
        String::new(),
    ];
    let mut set = StatsSet::new(&target.addrs);
    let mut recent = Vec::new();
    for seq in 1..=5 {
        for addr in [v4(), v6()] {
            let r = sample(addr, seq);
            set.record(&r);
            recent.push(
                r.connect
                    .filter(|_| r.ok())
                    .map(|d| d.as_secs_f64() * 1000.0),
            );
            let mut line = probe_line(&r, cfg.port, true);
            if !r.ok() {
                line.push_str(&format!(" ({})", r.addr));
            }
            lines.push(line);
        }
    }
    lines.push(status_line(&set, &recent, true, Some(104)));
    let cols = lines
        .iter()
        .map(|l| {
            parse_ansi(l, FG)
                .iter()
                .map(|r| r.col + r.text.chars().count())
                .max()
                .unwrap_or(0)
        })
        .max()
        .unwrap_or(80)
        .max(80);
    let rows: Vec<Vec<Run>> = lines.iter().map(|l| parse_ansi(l, FG)).collect();
    svg("paping one.one.one.one -p https --http", cols + 1, &rows)
}

/// `--tui` after a minute of probing, drawn through ratatui's test backend.
fn tui_image() -> String {
    const COLS: u16 = 112;
    const ROWS: u16 = 38;
    let mut cfg = config();
    cfg.output = OutputMode::Tui;
    let mut state = TuiState::new(target(), &cfg);
    state.started = Instant::now() - Duration::from_secs(61);
    for seq in 1..=61 {
        for addr in [v4(), v6()] {
            state.apply(sample(addr, seq));
        }
    }
    let mut terminal = Terminal::new(TestBackend::new(COLS, ROWS)).unwrap();
    terminal.draw(|frame| render(frame, &state)).unwrap();
    let buffer = terminal.backend().buffer();
    let mut rows = Vec::new();
    for y in 0..ROWS {
        let mut runs: Vec<Run> = Vec::new();
        for x in 0..COLS {
            let cell = &buffer[(x, y)];
            let fg = ratatui_color(cell.fg).unwrap_or(FG).to_string();
            let bg = ratatui_color(cell.bg).map(str::to_string);
            let bold = cell.modifier.contains(Modifier::BOLD);
            match runs.last_mut() {
                Some(run) if run.fg == fg && run.bg == bg && run.bold == bold => {
                    run.text.push_str(cell.symbol());
                }
                _ => runs.push(Run {
                    col: usize::from(x),
                    text: cell.symbol().to_string(),
                    fg,
                    bg,
                    bold,
                }),
            }
        }
        rows.push(runs);
    }
    svg(
        "paping one.one.one.one -p https --http --tui",
        usize::from(COLS),
        &rows,
    )
}

fn main() {
    fs::create_dir_all("docs/images").unwrap();
    fs::write("docs/images/terminal.svg", terminal_image()).unwrap();
    fs::write("docs/images/tui.svg", tui_image()).unwrap();
    println!("wrote docs/images/terminal.svg and docs/images/tui.svg");
}
