//! The original paping output, line for line. In the original strings the parts between
//! `$` signs were colored; `paint` marks the same parts here.
//!
//! On a terminal (`interactive`) there is more: a status line under the latest result
//! that redraws itself, and a trend line in the statistics. Pipes and files never see it.

use std::collections::VecDeque;
use std::fmt::Display;
use std::io::{self, Write};
use std::net::IpAddr;

use super::Sink;
use crate::cli::Config;
use crate::probe::ProbeResult;
use crate::resolve::Target;
use crate::stats::{Stats, StatsSet};

const YELLOW: u8 = 33;
const GREEN: u8 = 32;
pub(crate) const RED: u8 = 31;
const BLUE: u8 = 34;

/// Moves to the start of the line and clears it, to replace the status line.
pub(crate) const CLEAR_LINE: &str = "\r\x1b[2K";

/// How many results the status line's sparkline shows.
const STATUS_RESULTS: usize = 20;

const SPARKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub struct TextSink<W: Write> {
    out: W,
    color: bool,
    interactive: bool,
    /// Only used when interactive: what the status line shows.
    stats: StatsSet,
    recent: VecDeque<Option<f64>>,
    status_shown: bool,
    /// Columns the status line may use; asked again for every line, so resizing works.
    width: fn() -> Option<usize>,
}

impl<W: Write> TextSink<W> {
    pub fn new(out: W, color: bool, interactive: bool) -> Self {
        TextSink {
            out,
            color,
            interactive,
            stats: StatsSet::new(&[]),
            recent: VecDeque::new(),
            status_shown: false,
            width: terminal_width,
        }
    }

    #[cfg(test)]
    fn with_width(mut self, width: fn() -> Option<usize>) -> Self {
        self.width = width;
        self
    }

    fn clear_status(&mut self) -> io::Result<()> {
        if self.status_shown {
            self.out.write_all(CLEAR_LINE.as_bytes())?;
            self.status_shown = false;
        }
        Ok(())
    }
}

impl<W: Write> Sink for TextSink<W> {
    fn start(&mut self, target: &Target, cfg: &Config) -> io::Result<()> {
        self.stats = StatsSet::new(&target.addrs);
        writeln!(self.out, "{}", header(target, cfg.port, self.color))?;
        if let (true, Some(dns)) = (cfg.details, target.dns_time) {
            let dns = ms(dns.as_secs_f64() * 1000.0);
            writeln!(self.out, "DNS lookup: {}", paint(dns, YELLOW, self.color))?;
        }
        writeln!(self.out)?;
        self.out.flush()
    }

    fn probe(&mut self, r: &ProbeResult, _target: &Target, cfg: &Config) -> io::Result<()> {
        self.clear_status()?;
        let mut line = probe_line(r, cfg.port, self.color);
        // With two addresses a bare "Connection timed out" would not say which one.
        if !r.ok() && self.stats.len() > 1 {
            line.push_str(&format!(" ({})", r.addr));
        }
        writeln!(self.out, "{line}")?;
        if self.interactive {
            self.stats.record(r);
            if self.recent.len() == STATUS_RESULTS {
                self.recent.pop_front();
            }
            self.recent
                .push_back(r.connect.filter(|_| r.ok()).map(to_ms));
            let recent: Vec<Option<f64>> = self.recent.iter().copied().collect();
            write!(
                self.out,
                "{}",
                status_line(&self.stats, &recent, self.color, (self.width)())
            )?;
            self.status_shown = true;
        }
        self.out.flush()
    }

    fn finish(&mut self, stats: &StatsSet, cfg: &Config) -> io::Result<()> {
        self.clear_status()?;
        let blocks = stats_blocks(stats, cfg, self.color, self.interactive);
        writeln!(self.out, "{blocks}")?;
        self.out.flush()
    }
}

/// `Connecting to host [ip] on TCP port:`, or without the brackets for an IP literal.
/// A name with two addresses lists both: `[192.0.2.10, 2001:db8::5]`.
pub fn header(target: &Target, port: u16, color: bool) -> String {
    let y = |s: &dyn Display| paint(s, YELLOW, color);
    let proto = format!("TCP {port}");
    if target.is_literal {
        format!("Connecting to {} on {}:", y(&target.primary()), y(&proto))
    } else {
        let addrs: Vec<String> = target.addrs.iter().map(|a| y(a)).collect();
        format!(
            "Connecting to {} [{}] on {}:",
            y(&target.host),
            addrs.join(", "),
            y(&proto)
        )
    }
}

/// The line for one attempt: `Connected to …` in green, or the error message in red.
/// With colors, the times are colored by how good they are.
pub fn probe_line(r: &ProbeResult, port: u16, color: bool) -> String {
    let (Some(connect), None) = (r.connect, &r.error) else {
        let message = r.error.as_ref().map(|e| e.message()).unwrap_or_default();
        return paint(message, RED, color);
    };
    let g = |s: &dyn Display| paint(s, GREEN, color);
    let time = |ms_value: f64| paint(ms(ms_value), quality(ms_value), color);
    let mut line = format!(
        "Connected to {}: time={} protocol={} port={}",
        g(&r.addr),
        time(to_ms(connect)),
        g(&"TCP"),
        g(&port)
    );
    if let Some(tls) = r.tls {
        line.push_str(&format!(" tls={}", time(to_ms(tls))));
    }
    if let Some(status) = r.http_status {
        let status_color = match status {
            500.. => RED,
            400.. => YELLOW,
            _ => GREEN,
        };
        line.push_str(&format!(" http={}", paint(status, status_color, color)));
        if let Some(ttfb) = r.ttfb {
            line.push_str(&format!(" ttfb={}", time(to_ms(ttfb))));
        }
    }
    line
}

/// One statistics block per address; the address is named only when there are two.
pub fn stats_blocks(set: &StatsSet, cfg: &Config, color: bool, trend: bool) -> String {
    let label = set.len() > 1;
    set.iter()
        .map(|(addr, stats)| stats_block(stats, cfg, color, label.then_some(addr), trend))
        .collect()
}

/// The statistics block, starting with an empty line as in the original. Times show
/// `0.00ms` when nothing connected. `label` names the address: `Connection statistics (ip):`.
/// `trend` adds a sparkline of the latest attempts (terminal only).
pub fn stats_block(
    stats: &Stats,
    cfg: &Config,
    color: bool,
    label: Option<IpAddr>,
    trend: bool,
) -> String {
    let b = |s: &dyn Display| paint(s, BLUE, color);
    let t = |v: Option<f64>| b(&ms(v.unwrap_or(0.0)));
    let label = label.map(|addr| format!(" ({addr})")).unwrap_or_default();
    let mut block = format!(
        "\nConnection statistics{label}:\n\tAttempted = {}, Connected = {}, Failed = {} ({})\n\
         Approximate connection times:\n\tMinimum = {}, Maximum = {}, Average = {}\n",
        b(&stats.attempted()),
        b(&stats.connected()),
        b(&stats.failed()),
        b(&format!("{:.2}%", stats.loss_pct())),
        t(stats.min_ms()),
        t(stats.max_ms()),
        t(stats.avg_ms()),
    );
    if trend {
        let recent: Vec<Option<f64>> = stats.recent().iter().copied().collect();
        block.push_str(&format!("\tTrend: {}\n", sparkline(&recent, color)));
    }
    if cfg.details {
        block.push_str(&format!(
            "\tMedian = {}, Jitter = {}, Std. deviation = {}\n",
            t(stats.median_ms()),
            t(stats.jitter_ms()),
            t(stats.stddev_ms()),
        ));
        if cfg.tls {
            block.push_str(&format!(
                "Approximate TLS handshake times:\n\tMinimum = {}, Maximum = {}, Average = {}\n",
                t(stats.tls_min_ms()),
                t(stats.tls_max_ms()),
                t(stats.tls_avg_ms()),
            ));
        }
        if cfg.http {
            block.push_str(&format!(
                "Approximate HTTP response times:\n\tMinimum = {}, Maximum = {}, Average = {}\n",
                t(stats.ttfb_min_ms()),
                t(stats.ttfb_max_ms()),
                t(stats.ttfb_avg_ms()),
            ));
        }
    }
    block
}

/// `▁▄█×`: each value scaled between the smallest and largest present value; `×` for a
/// failed attempt. All equal values sit in the middle.
pub fn sparkline(values: &[Option<f64>], color: bool) -> String {
    let present = values.iter().flatten();
    let min = present.clone().copied().fold(f64::INFINITY, f64::min);
    let max = present.copied().fold(f64::NEG_INFINITY, f64::max);
    values
        .iter()
        .map(|value| match value {
            None => paint('×', RED, color),
            Some(v) => {
                let index = if max > min {
                    (((v - min) / (max - min)) * 7.0).floor() as usize
                } else {
                    3
                };
                paint(SPARKS[index.min(7)], GREEN, color)
            }
        })
        .collect()
}

/// `▁▂▁▃ 8 sent · 0 lost (0.0%) · avg 9.65 ms`; with two addresses the average is shown
/// per family: `· v4 avg 9.65 ms · v6 avg 10.12 ms`.
pub fn status_line(
    set: &StatsSet,
    recent: &[Option<f64>],
    color: bool,
    width: Option<usize>,
) -> String {
    let attempted = set.attempted();
    let lost = set.failed();
    let loss = if attempted == 0 {
        0.0
    } else {
        lost as f64 / attempted as f64 * 100.0
    };
    let avg = |v: Option<f64>| v.map_or("-".to_string(), |v| format!("{v:.2} ms"));
    let core = format!("{attempted} sent · {lost} lost ({loss:.1}%)");
    let mut averages = String::new();
    if set.len() > 1 {
        for (addr, stats) in set.iter() {
            let family = if addr.is_ipv4() { "v4" } else { "v6" };
            averages.push_str(&format!(" · {family} avg {}", avg(stats.avg_ms())));
        }
    } else if let Some((_, stats)) = set.iter().next() {
        averages.push_str(&format!(" · avg {}", avg(stats.avg_ms())));
    }

    // A wrapped status line cannot be cleared, so it must fit: drop the sparkline first,
    // then the averages, and cut what is left.
    let fits = |text: &str| width.is_none_or(|w| text.chars().count() <= w);
    let spark_width = recent.len() + 1;
    let with_averages = format!("{core}{averages}");
    if width.is_none_or(|w| spark_width + with_averages.chars().count() <= w) {
        return format!("{} {with_averages}", sparkline(recent, color));
    }
    if fits(&with_averages) {
        return with_averages;
    }
    if fits(&core) {
        return core;
    }
    core.chars().take(width.unwrap_or(0)).collect()
}

/// The terminal width minus one column, so the cursor never wraps.
fn terminal_width() -> Option<usize> {
    crossterm::terminal::size()
        .ok()
        .and_then(|(columns, _)| usable_width(columns))
}

/// `None` when the terminal does not know its size (0 columns).
fn usable_width(columns: u16) -> Option<usize> {
    (columns > 0).then(|| usize::from(columns) - 1)
}

/// Green under 50 ms, yellow under 150 ms, red above.
fn quality(ms_value: f64) -> u8 {
    if ms_value < 50.0 {
        GREEN
    } else if ms_value < 150.0 {
        YELLOW
    } else {
        RED
    }
}

fn to_ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn ms(value: f64) -> String {
    format!("{value:.2}ms")
}

pub(crate) fn paint(text: impl Display, code: u8, on: bool) -> String {
    if on {
        format!("\x1b[1;{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Family, OutputMode};
    use crate::probe::ProbeError;
    use jiff::Timestamp;
    use std::time::Duration;

    fn cfg() -> Config {
        Config {
            destination: "example.com".into(),
            port: 443,
            timeout: Duration::from_millis(1000),
            count: None,
            color: false,
            bind: None,
            family: Family::Any,
            interval: Duration::from_millis(1000),
            tls: false,
            details: false,
            output: OutputMode::Text,
            http: false,
            http_path: "/".into(),
        }
    }

    fn name_target() -> Target {
        Target {
            host: "example.com".into(),
            addrs: vec!["93.184.215.14".parse().unwrap()],
            is_literal: false,
            dns_time: Some(Duration::from_micros(12_340)),
        }
    }

    fn literal_target(ip: &str) -> Target {
        Target {
            host: ip.into(),
            addrs: vec![ip.parse().unwrap()],
            is_literal: true,
            dns_time: None,
        }
    }

    fn ok(ms: f64) -> ProbeResult {
        ProbeResult {
            seq: 1,
            at: Timestamp::UNIX_EPOCH,
            addr: "93.184.215.14".parse().unwrap(),
            http_status: None,
            ttfb: None,
            connect: Some(Duration::from_secs_f64(ms / 1000.0)),
            tls: None,
            error: None,
        }
    }

    fn failed(error: ProbeError) -> ProbeResult {
        ProbeResult {
            connect: None,
            error: Some(error),
            ..ok(0.0)
        }
    }

    fn stats(results: &[ProbeResult]) -> Stats {
        let mut s = Stats::default();
        for r in results {
            s.record(r);
        }
        s
    }

    fn ip() -> IpAddr {
        "93.184.215.14".parse().unwrap()
    }

    #[test]
    fn header_name() {
        assert_eq!(
            header(&name_target(), 443, false),
            "Connecting to example.com [93.184.215.14] on TCP 443:"
        );
    }

    #[test]
    fn header_literal() {
        assert_eq!(
            header(&literal_target("93.184.215.14"), 443, false),
            "Connecting to 93.184.215.14 on TCP 443:"
        );
        assert_eq!(
            header(&literal_target("::1"), 22, false),
            "Connecting to ::1 on TCP 22:"
        );
    }

    #[test]
    fn success_line() {
        assert_eq!(
            probe_line(&ok(13.39), 443, false),
            "Connected to 93.184.215.14: time=13.39ms protocol=TCP port=443"
        );
        let with_tls = ProbeResult {
            tls: Some(Duration::from_micros(4_210)),
            ..ok(13.39)
        };
        assert_eq!(
            probe_line(&with_tls, 443, false),
            "Connected to 93.184.215.14: time=13.39ms protocol=TCP port=443 tls=4.21ms"
        );
    }

    #[test]
    fn failure_lines() {
        for e in [
            ProbeError::Timeout,
            ProbeError::Refused,
            ProbeError::NetUnreachable,
            ProbeError::HostUnreachable,
            ProbeError::Tls("eof".into()),
            ProbeError::Other("boom".into()),
        ] {
            assert_eq!(probe_line(&failed(e.clone()), 443, false), e.message());
        }
    }

    #[test]
    fn stats_block_with_label() {
        let s = stats(&[ok(13.39)]);
        let block = stats_block(&s, &cfg(), false, Some("::1".parse().unwrap()), false);
        assert!(
            block.starts_with("\nConnection statistics (::1):\n\tAttempted = 1,"),
            "{block}"
        );
        let unlabeled = stats_block(&s, &cfg(), false, None, false);
        assert!(unlabeled.starts_with("\nConnection statistics:\n"));
    }

    #[test]
    fn stats_plain() {
        let s = stats(&[ok(13.39), failed(ProbeError::Timeout)]);
        assert_eq!(
            stats_block(&s, &cfg(), false, None, false),
            "\nConnection statistics:\n\tAttempted = 2, Connected = 1, Failed = 1 (50.00%)\nApproximate connection times:\n\tMinimum = 13.39ms, Maximum = 13.39ms, Average = 13.39ms\n"
        );
    }

    #[test]
    fn stats_no_success() {
        let s = stats(&[
            failed(ProbeError::Timeout),
            failed(ProbeError::Timeout),
            failed(ProbeError::Refused),
        ]);
        assert_eq!(
            stats_block(&s, &cfg(), false, None, false),
            "\nConnection statistics:\n\tAttempted = 3, Connected = 0, Failed = 3 (100.00%)\nApproximate connection times:\n\tMinimum = 0.00ms, Maximum = 0.00ms, Average = 0.00ms\n"
        );
    }

    #[test]
    fn stats_details() {
        let s = stats(&[ok(10.0), ok(20.0), ok(30.0)]);
        let details = Config {
            details: true,
            ..cfg()
        };
        assert_eq!(
            stats_block(&s, &details, false, None, false),
            "\nConnection statistics:\n\tAttempted = 3, Connected = 3, Failed = 0 (0.00%)\nApproximate connection times:\n\tMinimum = 10.00ms, Maximum = 30.00ms, Average = 20.00ms\n\tMedian = 20.00ms, Jitter = 10.00ms, Std. deviation = 8.16ms\n"
        );

        let with_tls = |ms, tls_ms| ProbeResult {
            tls: Some(Duration::from_secs_f64(tls_ms / 1000.0)),
            ..ok(ms)
        };
        let s = stats(&[with_tls(10.0, 4.0), with_tls(10.0, 6.0)]);
        let tls = Config {
            details: true,
            tls: true,
            ..cfg()
        };
        assert!(stats_block(&s, &tls, false, None, false).ends_with(
            "\tMedian = 10.00ms, Jitter = 0.00ms, Std. deviation = 0.00ms\nApproximate TLS handshake times:\n\tMinimum = 4.00ms, Maximum = 6.00ms, Average = 5.00ms\n"
        ));
        // Without --details there is no TLS block either.
        let tls_only = Config { tls: true, ..cfg() };
        assert!(!stats_block(&s, &tls_only, false, None, false).contains("TLS"));
    }

    #[test]
    fn colored_success() {
        assert_eq!(
            probe_line(&ok(13.39), 443, true),
            "Connected to \x1b[1;32m93.184.215.14\x1b[0m: time=\x1b[1;32m13.39ms\x1b[0m protocol=\x1b[1;32mTCP\x1b[0m port=\x1b[1;32m443\x1b[0m"
        );
        assert_eq!(
            probe_line(&failed(ProbeError::Timeout), 443, true),
            "\x1b[1;31mConnection timed out\x1b[0m"
        );
        assert!(
            header(&name_target(), 443, true)
                .starts_with("Connecting to \x1b[1;33mexample.com\x1b[0m [")
        );
        assert!(
            stats_block(&stats(&[ok(1.0)]), &cfg(), true, None, false)
                .contains("Attempted = \x1b[1;34m1\x1b[0m,")
        );
    }

    #[test]
    fn sink_sequence() {
        let details = Config {
            details: true,
            ..cfg()
        };
        let target = name_target();
        let r = ProbeResult {
            addr: ip(),
            ..ok(13.39)
        };
        let mut set = StatsSet::new(&target.addrs);
        set.record(&r);
        let mut sink = TextSink::new(Vec::new(), false, false);
        sink.start(&target, &details).unwrap();
        sink.probe(&r, &target, &details).unwrap();
        sink.finish(&set, &details).unwrap();
        let expected = format!(
            "Connecting to example.com [93.184.215.14] on TCP 443:\nDNS lookup: 12.34ms\n\n{}\n{}\n",
            probe_line(&r, 443, false),
            stats_block(
                &stats(std::slice::from_ref(&r)),
                &details,
                false,
                None,
                false
            )
        );
        assert_eq!(String::from_utf8(sink.out).unwrap(), expected);

        // No DNS line without --details, and none for literals.
        let mut sink = TextSink::new(Vec::new(), false, false);
        sink.start(&target, &cfg()).unwrap();
        assert_eq!(
            String::from_utf8(sink.out).unwrap(),
            "Connecting to example.com [93.184.215.14] on TCP 443:\n\n"
        );
        let mut sink = TextSink::new(Vec::new(), false, false);
        sink.start(&literal_target("127.0.0.1"), &details).unwrap();
        assert_eq!(
            String::from_utf8(sink.out).unwrap(),
            "Connecting to 127.0.0.1 on TCP 443:\n\n"
        );
    }

    fn v4() -> IpAddr {
        "192.0.2.10".parse().unwrap()
    }

    fn v6() -> IpAddr {
        "2001:db8::5".parse().unwrap()
    }

    fn dual_target() -> Target {
        Target {
            host: "example.com".into(),
            addrs: vec![v4(), v6()],
            is_literal: false,
            dns_time: Some(Duration::from_millis(12)),
        }
    }

    fn at(addr: IpAddr, r: ProbeResult) -> ProbeResult {
        ProbeResult { addr, ..r }
    }

    #[test]
    fn header_two_addrs() {
        assert_eq!(
            header(&dual_target(), 443, false),
            "Connecting to example.com [192.0.2.10, 2001:db8::5] on TCP 443:"
        );
    }

    #[test]
    fn http_suffix() {
        let r = ProbeResult {
            http_status: Some(200),
            ttfb: Some(Duration::from_micros(23_450)),
            ..ok(13.39)
        };
        assert_eq!(
            probe_line(&r, 443, false),
            "Connected to 93.184.215.14: time=13.39ms protocol=TCP port=443 http=200 ttfb=23.45ms"
        );
        let not_found = ProbeResult {
            http_status: Some(404),
            ..r.clone()
        };
        assert!(
            probe_line(&not_found, 443, true).contains(" http=\x1b[1;33m404\x1b[0m "),
            "{}",
            probe_line(&not_found, 443, true)
        );
    }

    #[test]
    fn quality_colors() {
        assert!(probe_line(&ok(5.0), 443, true).contains("time=\x1b[1;32m5.00ms\x1b[0m"));
        assert!(probe_line(&ok(80.0), 443, true).contains("time=\x1b[1;33m80.00ms\x1b[0m"));
        assert!(probe_line(&ok(200.0), 443, true).contains("time=\x1b[1;31m200.00ms\x1b[0m"));
        // The rest of the line stays green.
        assert!(probe_line(&ok(200.0), 443, true).contains("port=\x1b[1;32m443\x1b[0m"));
    }

    #[test]
    fn sparkline_scaling() {
        assert_eq!(
            sparkline(&[Some(1.0), Some(2.0), Some(3.0), None], false),
            "▁▄█×"
        );
        assert_eq!(
            sparkline(&[Some(1.0), None], true),
            "\x1b[1;32m▄\x1b[0m\x1b[1;31m×\x1b[0m"
        );
        assert_eq!(sparkline(&[], false), "");
    }

    #[test]
    fn sparkline_flat() {
        assert_eq!(sparkline(&[Some(5.0), Some(5.0), Some(5.0)], false), "▄▄▄");
    }

    #[test]
    fn status_line_single() {
        let addr: IpAddr = "93.184.215.14".parse().unwrap();
        let mut set = StatsSet::new(&[addr]);
        for r in [ok(10.0), ok(20.0), failed(ProbeError::Timeout)] {
            set.record(&r);
        }
        assert_eq!(
            status_line(&set, &[Some(10.0), Some(20.0), None], false, None),
            "▁█× 3 sent · 1 lost (33.3%) · avg 15.00 ms"
        );
    }

    #[test]
    fn status_line_dual() {
        let mut set = StatsSet::new(&[v4(), v6()]);
        set.record(&at(v4(), ok(9.0)));
        set.record(&at(v6(), ok(11.0)));
        assert_eq!(
            status_line(&set, &[Some(9.0), Some(11.0)], false, None),
            "▁█ 2 sent · 0 lost (0.0%) · v4 avg 9.00 ms · v6 avg 11.00 ms"
        );
        let mut empty_v6 = StatsSet::new(&[v4(), v6()]);
        empty_v6.record(&at(v4(), ok(9.0)));
        empty_v6.record(&at(v6(), failed(ProbeError::Refused)));
        assert!(status_line(&empty_v6, &[Some(9.0), None], false, None).ends_with("v6 avg -"));
    }

    #[test]
    fn failures_name_the_address_with_two_addresses() {
        let refused = |addr| at(addr, failed(ProbeError::Refused));
        let mut sink = TextSink::new(Vec::new(), false, false);
        sink.start(&dual_target(), &cfg()).unwrap();
        sink.probe(&at(v4(), ok(9.0)), &dual_target(), &cfg())
            .unwrap();
        sink.probe(&refused(v6()), &dual_target(), &cfg()).unwrap();
        let out = String::from_utf8(sink.out).unwrap();
        assert!(
            out.ends_with("\nConnection refused (2001:db8::5)\n"),
            "{out:?}"
        );
        assert!(
            out.contains("\nConnected to 192.0.2.10: time=9.00ms"),
            "{out:?}"
        );

        // A single address keeps the original line.
        let single = literal_target("192.0.2.10");
        let mut sink = TextSink::new(Vec::new(), false, false);
        sink.start(&single, &cfg()).unwrap();
        sink.probe(&refused(v4()), &single, &cfg()).unwrap();
        let out = String::from_utf8(sink.out).unwrap();
        assert!(out.ends_with("\nConnection refused\n"), "{out:?}");
    }

    #[test]
    fn zero_columns_means_unknown_width() {
        // A pty without a size (e.g. `script` under ssh without -t) reports 0 columns;
        // that must not cut the status line down to nothing.
        assert_eq!(usable_width(0), None);
        assert_eq!(usable_width(80), Some(79));
    }

    #[test]
    fn status_line_fits_the_terminal() {
        let mut set = StatsSet::new(&[v4(), v6()]);
        set.record(&at(v4(), ok(9.0)));
        set.record(&at(v6(), ok(11.0)));
        let recent = [Some(9.0), Some(11.0)];
        let full = "▁█ 2 sent · 0 lost (0.0%) · v4 avg 9.00 ms · v6 avg 11.00 ms";
        assert_eq!(status_line(&set, &recent, false, Some(60)), full);
        // The sparkline goes first …
        assert_eq!(
            status_line(&set, &recent, false, Some(59)),
            "2 sent · 0 lost (0.0%) · v4 avg 9.00 ms · v6 avg 11.00 ms"
        );
        // … then the averages …
        assert_eq!(
            status_line(&set, &recent, false, Some(40)),
            "2 sent · 0 lost (0.0%)"
        );
        // … and a very narrow terminal gets a cut line.
        assert_eq!(status_line(&set, &recent, false, Some(10)), "2 sent · 0");
        // Colors do not count toward the width.
        let colored = status_line(&set, &recent, true, Some(60));
        assert!(colored.contains("\x1b[1;32m▁\x1b[0m"), "{colored:?}");
    }

    #[test]
    fn interactive_sink_sequence() {
        let target = literal_target("93.184.215.14");
        let mut sink = TextSink::new(Vec::new(), false, true).with_width(|| None);
        sink.start(&target, &cfg()).unwrap();
        sink.probe(&ok(10.0), &target, &cfg()).unwrap();
        sink.probe(&ok(20.0), &target, &cfg()).unwrap();
        let mut set = StatsSet::new(&target.addrs);
        set.record(&ok(10.0));
        set.record(&ok(20.0));
        sink.finish(&set, &cfg()).unwrap();
        let out = String::from_utf8(sink.out).unwrap();
        let first = probe_line(&ok(10.0), 443, false);
        let second = probe_line(&ok(20.0), 443, false);
        assert!(
            out.starts_with(&format!(
                "Connecting to 93.184.215.14 on TCP 443:\n\n{first}\n▄ 1 sent"
            )),
            "{out:?}"
        );
        assert!(
            out.contains(&format!("{CLEAR_LINE}{second}\n▁█ 2 sent")),
            "{out:?}"
        );
        let tail = &out[out.rfind(CLEAR_LINE).unwrap() + CLEAR_LINE.len()..];
        assert!(tail.starts_with("\nConnection statistics:\n"), "{tail:?}");
        assert!(tail.contains("\tTrend: ▁█\n"), "{tail:?}");
        assert!(tail.ends_with("\n\n"), "{tail:?}");
    }

    #[test]
    fn non_interactive_has_no_extras() {
        let target = literal_target("93.184.215.14");
        let mut sink = TextSink::new(Vec::new(), false, false);
        sink.start(&target, &cfg()).unwrap();
        sink.probe(&ok(10.0), &target, &cfg()).unwrap();
        let mut set = StatsSet::new(&target.addrs);
        set.record(&ok(10.0));
        sink.finish(&set, &cfg()).unwrap();
        let out = String::from_utf8(sink.out).unwrap();
        assert!(!out.contains('\r'), "{out:?}");
        assert!(!out.contains("Trend:"), "{out:?}");
        assert!(!out.contains(" sent ·"), "{out:?}");
    }

    #[test]
    fn details_http_line() {
        let r = ProbeResult {
            http_status: Some(200),
            ttfb: Some(Duration::from_millis(20)),
            ..ok(10.0)
        };
        let http = Config {
            details: true,
            http: true,
            ..cfg()
        };
        let block = stats_block(&stats(&[r]), &http, false, None, false);
        assert!(
            block.ends_with(
                "Approximate HTTP response times:\n\tMinimum = 20.00ms, Maximum = 20.00ms, Average = 20.00ms\n"
            ),
            "{block}"
        );
        assert!(!stats_block(&stats(&[ok(1.0)]), &cfg(), false, None, false).contains("HTTP"));
    }
}
