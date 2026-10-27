//! Layout and widgets of the TUI. Pure: state in, frame out.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Bar, BarChart, BarGroup, Block, Chart, Dataset, GraphType, Paragraph,
};

use super::digits::big;
use super::{TuiState, family};
use crate::probe::ProbeResult;
use crate::stats::Stats;

/// Line colors per address: IPv4 green, IPv6 cyan.
const SERIES: [Color; 2] = [Color::Green, Color::Cyan];
const HISTOGRAM_BINS: usize = 8;

/// Header, last result and statistics, chart, log, key help; top to bottom.
pub fn render(frame: &mut Frame, state: &TuiState) {
    let n = state.target.addrs.len() as u16;
    // Tall enough for the big digits of every address and for every statistics row.
    let stats_rows = 7 + u16::from(n > 1) + u16::from(state.tls) + u16::from(state.http);
    let top_height = 2 + (4 * n).max(stats_rows);
    let [header, top, chart, log, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(top_height),
        Constraint::Min(6),
        Constraint::Length(8),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let [last, stats] =
        Layout::horizontal([Constraint::Length(36), Constraint::Min(30)]).areas(top);

    frame.render_widget(header_line(state), header);
    render_last(frame, last, state);
    render_stats(frame, stats, state);
    render_chart(frame, chart, state);
    render_log(frame, log, state);
    frame.render_widget(
        Line::styled(" q quit · p pause", Style::new().fg(Color::DarkGray)),
        footer,
    );
}

fn header_line(state: &TuiState) -> Line<'static> {
    let t = &state.target;
    let mut spans = vec![Span::styled(
        format!(" {} ", t.host),
        Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    )];
    if !t.is_literal {
        let addrs: Vec<String> = t.addrs.iter().map(ToString::to_string).collect();
        spans.push(Span::raw(format!("[{}] ", addrs.join(", "))));
    }
    let proto = match (state.tls, state.http) {
        (true, true) => "HTTPS",
        (false, true) => "HTTP",
        (true, false) => "TCP+TLS",
        (false, false) => "TCP",
    };
    spans.push(Span::raw(format!("{proto} {}", state.port)));
    if let Some(dns) = t.dns_time {
        spans.push(Span::raw(format!(" · DNS {:.2} ms", ms(dns))));
    }
    let secs = state.started.elapsed().as_secs();
    spans.push(Span::raw(format!(
        " · running {:02}:{:02}",
        secs / 60,
        secs % 60
    )));
    if state.paused {
        spans.push(Span::styled(
            "  PAUSED ",
            Style::new().fg(Color::Black).bg(Color::Yellow),
        ));
    }
    if state.finished {
        spans.push(Span::styled(
            "  DONE ",
            Style::new().fg(Color::Black).bg(Color::Blue),
        ));
    }
    Line::from(spans)
}

/// The latest connect time per address, in big digits when they fit.
fn render_last(frame: &mut Frame, area: Rect, state: &TuiState) {
    let block = Block::bordered().title(" Last ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let n = state.target.addrs.len();
    let fits_big = inner.width >= 22 && usize::from(inner.height) >= 4 * n;
    let mut lines = Vec::new();
    for (i, addr) in state.target.addrs.iter().enumerate() {
        let tag = family(*addr);
        match &state.last[i] {
            None => {
                lines.push(Line::from(format!("{tag} waiting…")));
                if fits_big {
                    lines.extend([Line::from(""), Line::from(""), Line::from("")]);
                }
            }
            Some(r) => match (&r.error, r.connect) {
                (None, Some(connect)) => {
                    let value = ms(connect);
                    let style = Style::new()
                        .fg(quality_color(value))
                        .add_modifier(Modifier::BOLD);
                    if fits_big {
                        for row in big(&format!("{value:.2}")) {
                            lines.push(Line::styled(row, style));
                        }
                        lines.push(Line::from(details(tag, r, true)));
                    } else {
                        lines.push(Line::from(vec![
                            Span::styled(format!("{value:.2} ms"), style),
                            Span::raw(format!(" {}", details(tag, r, false))),
                        ]));
                    }
                }
                (error, _) => {
                    let message = error.as_ref().map(|e| e.message()).unwrap_or_default();
                    lines.push(Line::styled(
                        format!("{tag} {message}"),
                        Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ));
                    if fits_big {
                        lines.extend([
                            Line::from(""),
                            Line::from(""),
                            Line::from(format!("#{}", r.seq)),
                        ]);
                    }
                }
            },
        }
    }
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

/// `v4 ms · TLS 11.73 · HTTP 200 · #60`
fn details(tag: &str, r: &ProbeResult, unit: bool) -> String {
    let mut text = if unit {
        format!("{tag} ms")
    } else {
        tag.to_string()
    };
    if let Some(tls) = r.tls {
        text.push_str(&format!(" · TLS {:.2}", ms(tls)));
    }
    if let Some(status) = r.http_status {
        text.push_str(&format!(" · HTTP {status}"));
    }
    text.push_str(&format!(" · #{}", r.seq));
    text
}

/// Left: the figures (one column per address). Right: percentiles and a histogram.
fn render_stats(frame: &mut Frame, area: Rect, state: &TuiState) {
    let block = Block::bordered().title(" Statistics ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [table, histogram] =
        Layout::horizontal([Constraint::Min(30), Constraint::Length(26)]).areas(inner);

    let columns: Vec<(&'static str, &Stats)> = state
        .stats
        .iter()
        .map(|(addr, stats)| (family(addr), stats))
        .collect();
    let dual = columns.len() > 1;
    // Share the width left of the labels between the address columns.
    let width = (usize::from(table.width).saturating_sub(12) / columns.len().max(1)).clamp(8, 20);
    let value = |v: Option<f64>| v.map_or("-".to_string(), |v| format!("{v:.2}"));
    let label = |text: &str| Span::styled(format!("{text:<12}"), Style::new().fg(Color::Blue));
    let row = |name: &str, cell: &dyn Fn(&Stats) -> String| {
        let mut spans = vec![label(name)];
        for (_, stats) in &columns {
            spans.push(Span::raw(format!("{:<width$}", cell(stats))));
        }
        Line::from(spans)
    };

    let mut lines = Vec::new();
    if dual {
        let mut spans = vec![label("")];
        for (i, (tag, _)) in columns.iter().enumerate() {
            spans.push(Span::styled(
                format!("{tag:<width$}"),
                Style::new().fg(SERIES[i % 2]).add_modifier(Modifier::BOLD),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines.push(row("Attempted", &|s| s.attempted().to_string()));
    lines.push(row("Failed", &|s| {
        format!("{} ({:.2}%)", s.failed(), s.loss_pct())
    }));
    lines.push(row("Min/Avg/Max", &|s| {
        format!(
            "{}/{}/{}",
            value(s.min_ms()),
            value(s.avg_ms()),
            value(s.max_ms())
        )
    }));
    lines.push(row("Median", &|s| value(s.median_ms())));
    lines.push(row("p95 / p99", &|s| {
        format!("{} / {}", value(s.p95_ms()), value(s.p99_ms()))
    }));
    lines.push(row("Jitter", &|s| value(s.jitter_ms())));
    lines.push(row("Std. dev.", &|s| value(s.stddev_ms())));
    if state.tls {
        lines.push(row("TLS avg", &|s| value(s.tls_avg_ms())));
    }
    if state.http {
        lines.push(row("TTFB avg", &|s| value(s.ttfb_avg_ms())));
    }
    frame.render_widget(Paragraph::new(lines), table);
    render_histogram(frame, histogram, state);
}

fn render_histogram(frame: &mut Frame, area: Rect, state: &TuiState) {
    let values: Vec<f64> = state.history.iter().flatten().flatten().copied().collect();
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let title = if values.is_empty() {
        " ms ".to_string()
    } else {
        format!(" {min:.1}–{max:.1} ms ")
    };
    let bars: Vec<Bar> = bins(&values, HISTOGRAM_BINS)
        .into_iter()
        .map(|count| Bar::default().value(count).text_value(String::new()))
        .collect();
    let chart = BarChart::default()
        .block(Block::bordered().title(title))
        .data(BarGroup::default().bars(&bars))
        .bar_width(2)
        .bar_gap(1)
        .bar_style(Style::new().fg(Color::Blue));
    frame.render_widget(chart, area);
}

fn render_chart(frame: &mut Frame, area: Rect, state: &TuiState) {
    // Braille draws two points per cell; show as many recent attempts as fit.
    let fit = usize::from(area.width.saturating_sub(10)).max(1) * 2;
    let mut lines: Vec<(usize, Vec<(f64, f64)>)> = Vec::new();
    let mut failures: Vec<f64> = Vec::new();
    let mut present: Vec<f64> = Vec::new();
    let mut x_min = f64::INFINITY;
    let mut x_max = f64::NEG_INFINITY;
    for (i, (_, stats)) in state.stats.iter().enumerate() {
        let history = &state.history[i];
        let shown = history.len().min(fit);
        let recent: Vec<Option<f64>> = history
            .iter()
            .skip(history.len() - shown)
            .copied()
            .collect();
        let first_seq = stats.attempted() + 1 - shown as u64;
        x_min = x_min.min(first_seq as f64);
        x_max = x_max.max((first_seq + shown as u64).saturating_sub(1) as f64);
        present.extend(recent.iter().flatten().copied());
        for (offset, value) in recent.iter().enumerate() {
            if value.is_none() {
                failures.push((first_seq + offset as u64) as f64);
            }
        }
        for segment in segments(&recent, first_seq) {
            lines.push((i, segment));
        }
    }
    if !x_min.is_finite() {
        x_min = 1.0;
    }
    let x_max = x_max.max(x_min + 1.0);
    let [y_low, y_high] = y_bounds(&present);
    let failure_points: Vec<(f64, f64)> = failures.iter().map(|x| (*x, y_high)).collect();

    let dual = state.target.addrs.len() > 1;
    let mut named = [false; 2];
    let mut datasets: Vec<Dataset> = lines
        .iter()
        .map(|(i, points)| {
            let mut dataset = Dataset::default()
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::new().fg(SERIES[i % 2]))
                .data(points);
            if dual && !named[i % 2] {
                named[i % 2] = true;
                dataset = dataset.name(if *i == 0 { "IPv4" } else { "IPv6" });
            }
            dataset
        })
        .collect();
    datasets.push(
        Dataset::default()
            .marker(Marker::Block)
            .graph_type(GraphType::Scatter)
            .style(Style::new().fg(Color::Red))
            .data(&failure_points),
    );
    let axis_style = Style::new().fg(Color::DarkGray);
    let chart = Chart::new(datasets)
        .block(Block::bordered().title(" Connect time (ms) "))
        .hidden_legend_constraints((Constraint::Ratio(1, 3), Constraint::Ratio(1, 2)))
        .x_axis(
            Axis::default()
                .bounds([x_min, x_max])
                .labels([format!("#{}", x_min as u64), format!("#{}", x_max as u64)])
                .style(axis_style),
        )
        .y_axis(
            Axis::default()
                .bounds([y_low, y_high])
                .labels([format!("{y_low:.1}"), format!("{y_high:.1}")])
                .style(axis_style),
        );
    frame.render_widget(chart, area);
}

/// `#60  v4   9.03 ms  tls 11.73  http 200`, or the error in red.
fn render_log(frame: &mut Frame, area: Rect, state: &TuiState) {
    let fit = usize::from(area.height.saturating_sub(2));
    let lines: Vec<Line> = state
        .log
        .iter()
        .skip(state.log.len().saturating_sub(fit))
        .map(|entry| {
            let r = &entry.result;
            let prefix = format!("#{:<5} {:<3} ", r.seq, entry.family);
            match (&r.error, r.connect) {
                (None, Some(connect)) => {
                    let value = ms(connect);
                    let mut spans = vec![
                        Span::raw(prefix),
                        Span::styled(
                            format!("{value:>8.2} ms"),
                            Style::new().fg(quality_color(value)),
                        ),
                    ];
                    if let Some(tls) = r.tls {
                        spans.push(Span::raw(format!("  tls {:.2}", ms(tls))));
                    }
                    if let Some(status) = r.http_status {
                        spans.push(Span::raw(format!("  http {status}")));
                    }
                    Line::from(spans)
                }
                (error, _) => Line::from(vec![
                    Span::raw(prefix),
                    Span::styled(
                        error.as_ref().map(|e| e.message()).unwrap_or_default(),
                        Style::new().fg(Color::Red),
                    ),
                ]),
            }
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Log ")),
        area,
    );
}

/// Runs of consecutive successes as chart points `(seq, ms)`; a failure ends a run, so the
/// line has a gap there.
pub(crate) fn segments(values: &[Option<f64>], first_seq: u64) -> Vec<Vec<(f64, f64)>> {
    let mut runs = Vec::new();
    let mut current = Vec::new();
    for (offset, value) in values.iter().enumerate() {
        match value {
            Some(ms) => current.push(((first_seq + offset as u64) as f64, *ms)),
            None if !current.is_empty() => runs.push(std::mem::take(&mut current)),
            None => {}
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

/// The y-axis range: 80 % of the smallest to 120 % of the largest value; `[0, 1]` without data.
pub(crate) fn y_bounds(values: &[f64]) -> [f64; 2] {
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !max.is_finite() || max <= 0.0 {
        return [0.0, 1.0];
    }
    [min * 0.8, max * 1.2]
}

/// How many values fall into each of `n` equal classes between min and max.
pub(crate) fn bins(values: &[f64], n: usize) -> Vec<u64> {
    let mut counts = vec![0; n];
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    for v in values {
        let index = if max > min {
            (((v - min) / (max - min)) * n as f64) as usize
        } else {
            n / 2
        };
        counts[index.min(n - 1)] += 1;
    }
    counts
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn quality_color(value: f64) -> Color {
    if value < 50.0 {
        Color::Green
    } else if value < 150.0 {
        Color::Yellow
    } else {
        Color::Red
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::tests::{ok, state, timeout, v4, v6};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn screen(state: &TuiState, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render(f, state)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn render_single() {
        let mut s = state(vec![v4()]);
        s.apply(ok(v4(), 1, 13.39));
        s.apply(ok(v4(), 2, 14.0));
        s.apply(timeout(v4(), 3));
        let text = screen(&s, 100, 30);
        assert!(text.contains("example.com"), "{text}");
        assert!(text.contains("192.0.2.10"), "{text}");
        assert!(text.contains("33.33%"), "{text}");
        assert!(text.contains("Connection timed out"), "{text}");
        assert!(text.contains("p95 / p99"), "{text}");
        assert!(text.contains("q quit"), "{text}");
    }

    #[test]
    fn render_big_digits_for_last() {
        let mut s = state(vec![v4()]);
        s.apply(ok(v4(), 1, 13.39));
        let text = screen(&s, 100, 30);
        assert!(text.contains(&big("13.39")[0]), "{text}");
        assert!(text.contains("v4 ms · #1"), "{text}");
    }

    #[test]
    fn render_dual_stack() {
        let mut s = state(vec![v4(), v6()]);
        s.apply(ok(v4(), 1, 9.0));
        s.apply(ok(v6(), 1, 11.0));
        let text = screen(&s, 110, 34);
        assert!(text.contains("[192.0.2.10, 2001:db8::5]"), "{text}");
        assert!(text.contains("#1     v6"), "{text}");
        assert!(text.contains("IPv6"), "{text}");
        let columns = text.lines().find(|l| l.contains("Min/Avg/Max")).unwrap();
        assert!(columns.contains("9.00/9.00/9.00") && columns.contains("11.00/11.00/11.00"));
    }

    #[test]
    fn render_paused_and_finished() {
        let mut s = state(vec![v4()]);
        s.paused = true;
        assert!(screen(&s, 100, 30).contains("PAUSED"));
        s.paused = false;
        s.finished = true;
        assert!(screen(&s, 100, 30).contains("DONE"));
    }

    #[test]
    fn render_empty_state() {
        assert!(screen(&state(vec![v4(), v6()]), 100, 30).contains("waiting"));
    }

    #[test]
    fn render_small_dual_does_not_panic() {
        let mut s = state(vec![v4(), v6()]);
        s.apply(ok(v4(), 1, 13.39));
        s.apply(timeout(v6(), 1));
        screen(&s, 40, 12);
        screen(&s, 20, 5);
        screen(&s, 1, 1);
    }

    #[test]
    fn chart_breaks_at_failure() {
        assert_eq!(
            segments(&[Some(1.0), None, Some(2.0), Some(3.0)], 10),
            vec![vec![(10.0, 1.0)], vec![(12.0, 2.0), (13.0, 3.0)]]
        );
        assert!(segments(&[None, None], 1).is_empty());
    }

    #[test]
    fn y_bounds_auto() {
        let [low, high] = y_bounds(&[9.0, 10.0]);
        assert!(
            (low - 7.2).abs() < 1e-9 && (high - 12.0).abs() < 1e-9,
            "{low} {high}"
        );
        assert_eq!(y_bounds(&[]), [0.0, 1.0]);
        assert_eq!(y_bounds(&[0.0]), [0.0, 1.0]);
    }

    #[test]
    fn histogram_bins() {
        let values = [1.0, 2.0, 2.5, 8.0, 8.0, 9.0];
        let counts = bins(&values, 8);
        assert_eq!(counts.len(), 8);
        assert_eq!(counts.iter().sum::<u64>(), 6);
        assert_eq!(counts[0], 1); // 1.0
        assert_eq!(counts[1], 2); // 2.0, 2.5
        assert_eq!(counts[7], 3); // 8.0, 8.0 and the maximum 9.0
        assert_eq!(bins(&[5.0, 5.0], 8).iter().sum::<u64>(), 2);
        assert_eq!(bins(&[], 8), vec![0; 8]);
    }
}
