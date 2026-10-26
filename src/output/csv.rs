//! CSV: a header line and one row per attempt; no summary.

use std::borrow::Cow;
use std::io::{self, Write};
use std::time::Duration;

use super::{Sink, timestamp};
use crate::cli::Config;
use crate::probe::ProbeResult;
use crate::resolve::Target;
use crate::stats::StatsSet;

pub struct CsvSink<W: Write> {
    out: W,
}

impl<W: Write> CsvSink<W> {
    pub fn new(out: W) -> Self {
        CsvSink { out }
    }
}

impl<W: Write> Sink for CsvSink<W> {
    fn start(&mut self, _target: &Target, _cfg: &Config) -> io::Result<()> {
        writeln!(
            self.out,
            "seq,time,ip,port,ok,connect_ms,tls_ms,error,http_status,ttfb_ms"
        )?;
        self.out.flush()
    }

    fn probe(&mut self, r: &ProbeResult, _target: &Target, cfg: &Config) -> io::Result<()> {
        let ms = |d: Option<Duration>| {
            d.map(|d| format!("{:.3}", d.as_secs_f64() * 1000.0))
                .unwrap_or_default()
        };
        writeln!(
            self.out,
            "{},{},{},{},{},{},{},{},{},{}",
            r.seq,
            timestamp(r.at),
            r.addr,
            cfg.port,
            r.ok(),
            ms(r.connect),
            ms(r.tls),
            csv_field(r.error.as_ref().map_or("", |e| e.code())),
            r.http_status.map(|s| s.to_string()).unwrap_or_default(),
            ms(r.ttfb),
        )?;
        self.out.flush()
    }

    fn finish(&mut self, _stats: &StatsSet, _cfg: &Config) -> io::Result<()> {
        self.out.flush()
    }
}

/// Quotes a field per RFC 4180 when it contains a comma, a quote or a line break.
pub fn csv_field(value: &str) -> Cow<'_, str> {
    if value.contains([',', '"', '\n', '\r']) {
        Cow::Owned(format!("\"{}\"", value.replace('"', "\"\"")))
    } else {
        Cow::Borrowed(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Family, OutputMode};
    use crate::probe::ProbeError;
    use std::time::Duration;

    fn cfg() -> Config {
        Config {
            destination: "127.0.0.1".into(),
            port: 80,
            timeout: Duration::from_millis(1000),
            count: None,
            color: false,
            bind: None,
            family: Family::Any,
            interval: Duration::from_millis(1000),
            tls: false,
            details: false,
            output: OutputMode::Csv,
            http: false,
            http_path: "/".into(),
        }
    }

    fn target() -> Target {
        Target {
            host: "127.0.0.1".into(),
            addrs: vec!["127.0.0.1".parse().unwrap()],
            is_literal: true,
            dns_time: None,
        }
    }

    fn ok(seq: u64) -> ProbeResult {
        ProbeResult {
            seq,
            at: "2026-10-26T19:00:00.123Z".parse().unwrap(),
            addr: std::net::Ipv4Addr::LOCALHOST.into(),
            http_status: None,
            ttfb: None,
            connect: Some(Duration::from_micros(13_390)),
            tls: None,
            error: None,
        }
    }

    #[test]
    fn rows() {
        let mut sink = CsvSink::new(Vec::new());
        let mut stats = StatsSet::new(&target().addrs);
        let results = [
            ok(1),
            ProbeResult {
                seq: 2,
                connect: None,
                error: Some(ProbeError::Refused),
                ..ok(2)
            },
            ProbeResult {
                tls: Some(Duration::from_micros(4_210)),
                ..ok(3)
            },
        ];
        sink.start(&target(), &cfg()).unwrap();
        for r in &results {
            stats.record(r);
            sink.probe(r, &target(), &cfg()).unwrap();
        }
        sink.finish(&stats, &cfg()).unwrap();
        assert_eq!(
            String::from_utf8(sink.out).unwrap(),
            "seq,time,ip,port,ok,connect_ms,tls_ms,error,http_status,ttfb_ms\n\
             1,2026-10-26T19:00:00.123Z,127.0.0.1,80,true,13.390,,,,\n\
             2,2026-10-26T19:00:00.123Z,127.0.0.1,80,false,,,refused,,\n\
             3,2026-10-26T19:00:00.123Z,127.0.0.1,80,true,13.390,4.210,,,\n"
        );
    }

    #[test]
    fn csv_http_columns() {
        let mut sink = CsvSink::new(Vec::new());
        let r = ProbeResult {
            addr: "::1".parse().unwrap(),
            http_status: Some(503),
            ttfb: Some(Duration::from_micros(23_450)),
            error: Some(ProbeError::HttpStatus(503)),
            ..ok(1)
        };
        sink.start(&target(), &cfg()).unwrap();
        sink.probe(&r, &target(), &cfg()).unwrap();
        let out = String::from_utf8(sink.out).unwrap();
        assert_eq!(
            out.lines().nth(1).unwrap(),
            "1,2026-10-26T19:00:00.123Z,::1,80,false,13.390,,http_status,503,23.450"
        );
    }

    #[test]
    fn quoting() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("a\"b"), "\"a\"\"b\"");
        assert_eq!(csv_field("a\nb"), "\"a\nb\"");
    }
}
