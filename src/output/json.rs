//! NDJSON: one `start` line, one `probe` line per attempt, one `summary` line per address.

use std::io::{self, Write};

use serde::Serialize;

use super::{Sink, ms_rounded, round_to, timestamp};
use crate::cli::Config;
use crate::probe::ProbeResult;
use crate::resolve::Target;
use crate::stats::StatsSet;

pub struct JsonSink<W: Write> {
    out: W,
}

impl<W: Write> JsonSink<W> {
    pub fn new(out: W) -> Self {
        JsonSink { out }
    }
}

impl<W: Write> Sink for JsonSink<W> {
    fn start(&mut self, target: &Target, cfg: &Config) -> io::Result<()> {
        self.line(&Start {
            kind: "start",
            version: env!("CARGO_PKG_VERSION"),
            destination: &target.host,
            ip: target.primary().to_string(),
            ips: target.addrs.iter().map(ToString::to_string).collect(),
            port: cfg.port,
            protocol: "TCP",
            tls: cfg.tls,
            dns_ms: target.dns_time.map(ms_rounded),
        })
    }

    fn probe(&mut self, r: &ProbeResult, _target: &Target, _cfg: &Config) -> io::Result<()> {
        self.line(&Probe {
            kind: "probe",
            seq: r.seq,
            ip: r.addr.to_string(),
            time: timestamp(r.at),
            ok: r.ok(),
            connect_ms: r.connect.map(ms_rounded),
            tls_ms: r.tls.map(ms_rounded),
            http_status: r.http_status,
            ttfb_ms: r.ttfb.map(ms_rounded),
            error: r.error.as_ref().map(|e| e.code()),
            error_detail: r.error.as_ref().and_then(|e| e.detail()),
        })
    }

    /// One summary line per address.
    fn finish(&mut self, set: &StatsSet, _cfg: &Config) -> io::Result<()> {
        let r = |v: Option<f64>| v.map(|v| round_to(v, 3));
        for (addr, stats) in set.iter() {
            self.line(&Summary {
                kind: "summary",
                ip: addr.to_string(),
                attempted: stats.attempted(),
                connected: stats.connected(),
                failed: stats.failed(),
                loss_pct: round_to(stats.loss_pct(), 2),
                min_ms: r(stats.min_ms()),
                max_ms: r(stats.max_ms()),
                avg_ms: r(stats.avg_ms()),
                median_ms: r(stats.median_ms()),
                p95_ms: r(stats.p95_ms()),
                p99_ms: r(stats.p99_ms()),
                jitter_ms: r(stats.jitter_ms()),
                stddev_ms: r(stats.stddev_ms()),
                tls_avg_ms: r(stats.tls_avg_ms()),
                ttfb_avg_ms: r(stats.ttfb_avg_ms()),
            })?;
        }
        Ok(())
    }
}

impl<W: Write> JsonSink<W> {
    fn line(&mut self, value: &impl Serialize) -> io::Result<()> {
        serde_json::to_writer(&mut self.out, value)?;
        writeln!(self.out)?;
        self.out.flush()
    }
}

#[derive(Serialize)]
struct Start<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    version: &'static str,
    destination: &'a str,
    ip: String,
    ips: Vec<String>,
    port: u16,
    protocol: &'static str,
    tls: bool,
    dns_ms: Option<f64>,
}

#[derive(Serialize)]
struct Probe {
    #[serde(rename = "type")]
    kind: &'static str,
    seq: u64,
    ip: String,
    time: String,
    ok: bool,
    connect_ms: Option<f64>,
    tls_ms: Option<f64>,
    http_status: Option<u16>,
    ttfb_ms: Option<f64>,
    error: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_detail: Option<String>,
}

#[derive(Serialize)]
struct Summary {
    #[serde(rename = "type")]
    kind: &'static str,
    ip: String,
    attempted: u64,
    connected: u64,
    failed: u64,
    loss_pct: f64,
    min_ms: Option<f64>,
    max_ms: Option<f64>,
    avg_ms: Option<f64>,
    median_ms: Option<f64>,
    p95_ms: Option<f64>,
    p99_ms: Option<f64>,
    jitter_ms: Option<f64>,
    stddev_ms: Option<f64>,
    tls_avg_ms: Option<f64>,
    ttfb_avg_ms: Option<f64>,
}

/// The line printed instead of the run when the destination cannot be resolved.
pub fn json_error_line(message: &str) -> String {
    serde_json::json!({"type": "error", "error": "resolve", "message": message}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Family, OutputMode};
    use crate::probe::ProbeError;
    use jiff::Timestamp;
    use serde_json::Value;
    use std::time::Duration;

    fn cfg(tls: bool) -> Config {
        Config {
            destination: "example.com".into(),
            port: 443,
            timeout: Duration::from_millis(1000),
            count: None,
            color: false,
            bind: None,
            family: Family::Any,
            interval: Duration::from_millis(1000),
            tls,
            details: false,
            output: OutputMode::Json,
            http: false,
            http_path: "/".into(),
        }
    }

    fn target() -> Target {
        Target {
            host: "example.com".into(),
            addrs: vec!["93.184.215.14".parse().unwrap()],
            is_literal: false,
            dns_time: Some(Duration::from_micros(12_340)),
        }
    }

    fn at() -> Timestamp {
        "2026-10-26T19:00:00.123Z".parse().unwrap()
    }

    fn ok() -> ProbeResult {
        ProbeResult {
            seq: 1,
            at: at(),
            addr: "93.184.215.14".parse().unwrap(),
            http_status: None,
            ttfb: None,
            connect: Some(Duration::from_micros(13_390)),
            tls: None,
            error: None,
        }
    }

    /// Writes one run and returns its lines.
    fn run(results: &[ProbeResult], tls: bool) -> Vec<String> {
        let mut sink = JsonSink::new(Vec::new());
        let mut stats = StatsSet::new(&target().addrs);
        sink.start(&target(), &cfg(tls)).unwrap();
        for r in results {
            stats.record(r);
            sink.probe(r, &target(), &cfg(tls)).unwrap();
        }
        sink.finish(&stats, &cfg(tls)).unwrap();
        String::from_utf8(sink.out)
            .unwrap()
            .lines()
            .map(String::from)
            .collect()
    }

    /// Asserts that `line` is a JSON object with exactly these keys, in this order.
    fn assert_keys(line: &str, keys: &[&str]) -> Value {
        let value: Value = serde_json::from_str(line).unwrap();
        assert_eq!(value.as_object().unwrap().len(), keys.len(), "{line}");
        let positions: Vec<usize> = keys
            .iter()
            .map(|k| {
                line.find(&format!("\"{k}\":"))
                    .unwrap_or_else(|| panic!("{k} missing in {line}"))
            })
            .collect();
        assert!(positions.is_sorted(), "key order in {line}");
        value
    }

    const PROBE_KEYS: [&str; 10] = [
        "type",
        "seq",
        "ip",
        "time",
        "ok",
        "connect_ms",
        "tls_ms",
        "http_status",
        "ttfb_ms",
        "error",
    ];

    #[test]
    fn start_line() {
        let lines = run(&[], false);
        let v = assert_keys(
            &lines[0],
            &[
                "type",
                "version",
                "destination",
                "ip",
                "ips",
                "port",
                "protocol",
                "tls",
                "dns_ms",
            ],
        );
        assert_eq!(v["type"], "start");
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(v["destination"], "example.com");
        assert_eq!(v["ip"], "93.184.215.14");
        assert_eq!(v["ips"], serde_json::json!(["93.184.215.14"]));
        assert_eq!(v["port"], 443);
        assert_eq!(v["protocol"], "TCP");
        assert_eq!(v["tls"], false);
        assert_eq!(v["dns_ms"], 12.34);
    }

    #[test]
    fn probe_lines() {
        let failed = ProbeResult {
            seq: 2,
            connect: None,
            error: Some(ProbeError::Timeout),
            ..ok()
        };
        let other = ProbeResult {
            seq: 3,
            connect: None,
            error: Some(ProbeError::Other("boom".into())),
            ..ok()
        };
        let lines = run(&[ok(), failed, other], false);

        let v = assert_keys(&lines[1], &PROBE_KEYS);
        assert_eq!(v["type"], "probe");
        assert_eq!(v["seq"], 1);
        assert_eq!(v["ip"], "93.184.215.14");
        assert_eq!(v["time"], "2026-10-26T19:00:00.123Z");
        assert_eq!(v["http_status"], Value::Null);
        assert_eq!(v["ttfb_ms"], Value::Null);
        assert_eq!(v["ok"], true);
        assert_eq!(v["connect_ms"], 13.39);
        assert_eq!(v["tls_ms"], Value::Null);
        assert_eq!(v["error"], Value::Null);

        let v = assert_keys(&lines[2], &PROBE_KEYS);
        assert_eq!(v["ok"], false);
        assert_eq!(v["connect_ms"], Value::Null);
        assert_eq!(v["error"], "timeout");

        let mut keys = PROBE_KEYS.to_vec();
        keys.push("error_detail");
        let v = assert_keys(&lines[3], &keys);
        assert_eq!(v["error"], "other");
        assert_eq!(v["error_detail"], "boom");
    }

    #[test]
    fn summary_line() {
        let lines = run(&[ok()], false);
        let v = assert_keys(
            lines.last().unwrap(),
            &[
                "type",
                "ip",
                "attempted",
                "connected",
                "failed",
                "loss_pct",
                "min_ms",
                "max_ms",
                "avg_ms",
                "median_ms",
                "p95_ms",
                "p99_ms",
                "jitter_ms",
                "stddev_ms",
                "tls_avg_ms",
                "ttfb_avg_ms",
            ],
        );
        assert_eq!(v["type"], "summary");
        assert_eq!(v["ip"], "93.184.215.14");
        assert_eq!(v["p95_ms"], 13.39);
        assert_eq!(v["attempted"], 1);
        assert_eq!(v["connected"], 1);
        assert_eq!(v["failed"], 0);
        assert_eq!(v["loss_pct"], 0.0);
        assert_eq!(v["avg_ms"], 13.39);
        assert_eq!(v["jitter_ms"], 0.0);
        assert_eq!(v["tls_avg_ms"], Value::Null);
    }

    #[test]
    fn summary_without_success_has_nulls() {
        let failed = ProbeResult {
            connect: None,
            error: Some(ProbeError::Refused),
            ..ok()
        };
        let lines = run(&[failed.clone(), failed.clone(), failed], false);
        let v: Value = serde_json::from_str(lines.last().unwrap()).unwrap();
        assert_eq!(v["loss_pct"], 100.0);
        for key in [
            "min_ms",
            "max_ms",
            "avg_ms",
            "median_ms",
            "jitter_ms",
            "stddev_ms",
        ] {
            assert_eq!(v[key], Value::Null, "{key}");
        }
    }

    #[test]
    fn tls_values() {
        let r = ProbeResult {
            tls: Some(Duration::from_micros(4_210)),
            ..ok()
        };
        let lines = run(&[r], true);
        let start: Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(start["tls"], true);
        let probe: Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(probe["tls_ms"], 4.21);
        let summary: Value = serde_json::from_str(&lines[2]).unwrap();
        assert_eq!(summary["tls_avg_ms"], 4.21);
    }

    #[test]
    fn tls_failure_keeps_connect_time() {
        let r = ProbeResult {
            error: Some(ProbeError::Tls("eof".into())),
            ..ok()
        };
        let lines = run(&[r], true);
        let probe: Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(probe["ok"], false);
        assert_eq!(probe["connect_ms"], 13.39);
        assert_eq!(probe["error"], "tls");
        assert_eq!(probe["error_detail"], "eof");
    }

    #[test]
    fn two_summaries_for_two_addrs() {
        let v6: std::net::IpAddr = "2001:db8::1".parse().unwrap();
        let dual = Target {
            addrs: vec!["93.184.215.14".parse().unwrap(), v6],
            ..target()
        };
        let mut sink = JsonSink::new(Vec::new());
        let mut stats = StatsSet::new(&dual.addrs);
        sink.start(&dual, &cfg(false)).unwrap();
        for r in [ok(), ProbeResult { addr: v6, ..ok() }] {
            stats.record(&r);
            sink.probe(&r, &dual, &cfg(false)).unwrap();
        }
        sink.finish(&stats, &cfg(false)).unwrap();
        let lines: Vec<Value> = String::from_utf8(sink.out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            lines[0]["ips"],
            serde_json::json!(["93.184.215.14", "2001:db8::1"])
        );
        assert_eq!(lines[2]["ip"], "2001:db8::1");
        let summaries: Vec<&Value> = lines.iter().filter(|l| l["type"] == "summary").collect();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0]["ip"], "93.184.215.14");
        assert_eq!(summaries[1]["ip"], "2001:db8::1");
    }

    #[test]
    fn http_fields() {
        let r = ProbeResult {
            http_status: Some(200),
            ttfb: Some(Duration::from_micros(23_450)),
            ..ok()
        };
        let lines = run(&[r], false);
        let probe: Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(probe["http_status"], 200);
        assert_eq!(probe["ttfb_ms"], 23.45);
        let summary: Value = serde_json::from_str(&lines[2]).unwrap();
        assert_eq!(summary["ttfb_avg_ms"], 23.45);

        let failed = ProbeResult {
            http_status: Some(503),
            error: Some(ProbeError::HttpStatus(503)),
            ..ok()
        };
        let lines = run(&[failed], false);
        let probe: Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(probe["error"], "http_status");
        assert_eq!(probe["error_detail"], "503");
        assert_eq!(probe["http_status"], 503);
    }

    #[test]
    fn loss_is_rounded() {
        let failed = ProbeResult {
            connect: None,
            error: Some(ProbeError::Timeout),
            ..ok()
        };
        let lines = run(&[ok(), ok(), failed], false);
        let v: Value = serde_json::from_str(lines.last().unwrap()).unwrap();
        assert_eq!(v["loss_pct"], 33.33);
    }

    #[test]
    fn error_line() {
        let v: Value = serde_json::from_str(&json_error_line("Cannot resolve host")).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"type": "error", "error": "resolve", "message": "Cannot resolve host"})
        );
    }
}
