use std::collections::VecDeque;
use std::net::IpAddr;

use crate::probe::ProbeResult;

/// How many connect times are kept for the median and standard deviation.
pub const SAMPLE_WINDOW: usize = 100_000;
/// How many recent attempts the trend shows.
pub const TREND: usize = 40;

/// Min, max and sum of one series of times in ms.
#[derive(Debug, Clone, Default)]
struct Series {
    count: u64,
    min: f64,
    max: f64,
    sum: f64,
}

impl Series {
    fn add(&mut self, ms: f64) {
        if self.count == 0 {
            self.min = ms;
            self.max = ms;
        } else {
            self.min = self.min.min(ms);
            self.max = self.max.max(ms);
        }
        self.count += 1;
        self.sum += ms;
    }

    fn min(&self) -> Option<f64> {
        (self.count > 0).then_some(self.min)
    }

    fn max(&self) -> Option<f64> {
        (self.count > 0).then_some(self.max)
    }

    fn avg(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum / self.count as f64)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    attempted: u64,
    connect: Series,
    tls: Series,
    window: VecDeque<f64>,
    last_connect: Option<f64>,
    jitter_sum: f64,
    jitter_count: u64,
    ttfb: Series,
    recent: VecDeque<Option<f64>>,
}

impl Stats {
    pub fn record(&mut self, r: &ProbeResult) {
        self.attempted += 1;
        let connect_ms = r
            .connect
            .filter(|_| r.ok())
            .map(|d| d.as_secs_f64() * 1000.0);
        if self.recent.len() == TREND {
            self.recent.pop_front();
        }
        self.recent.push_back(connect_ms);
        let Some(ms) = connect_ms else { return };
        self.connect.add(ms);
        if let Some(last) = self.last_connect.replace(ms) {
            self.jitter_sum += (ms - last).abs();
            self.jitter_count += 1;
        }
        if self.window.len() == SAMPLE_WINDOW {
            self.window.pop_front();
        }
        self.window.push_back(ms);
        if let Some(tls) = r.tls {
            self.tls.add(tls.as_secs_f64() * 1000.0);
        }
        if let Some(ttfb) = r.ttfb {
            self.ttfb.add(ttfb.as_secs_f64() * 1000.0);
        }
    }

    pub fn attempted(&self) -> u64 {
        self.attempted
    }

    pub fn connected(&self) -> u64 {
        self.connect.count
    }

    pub fn failed(&self) -> u64 {
        self.attempted - self.connected()
    }

    pub fn loss_pct(&self) -> f64 {
        if self.attempted == 0 {
            0.0
        } else {
            self.failed() as f64 / self.attempted as f64 * 100.0
        }
    }

    pub fn min_ms(&self) -> Option<f64> {
        self.connect.min()
    }

    pub fn max_ms(&self) -> Option<f64> {
        self.connect.max()
    }

    /// Mean over successful attempts. The original divided by all attempts, so every
    /// failure pulled the value down.
    pub fn avg_ms(&self) -> Option<f64> {
        self.connect.avg()
    }

    pub fn median_ms(&self) -> Option<f64> {
        if self.window.is_empty() {
            return None;
        }
        let mut values: Vec<f64> = self.window.iter().copied().collect();
        let len = values.len();
        let (lower, upper, _) = values.select_nth_unstable_by(len / 2, f64::total_cmp);
        if len % 2 == 1 {
            Some(*upper)
        } else {
            let below = lower.iter().copied().fold(f64::MIN, f64::max);
            Some((below + *upper) / 2.0)
        }
    }

    /// Mean absolute difference between consecutive successful connect times.
    pub fn jitter_ms(&self) -> Option<f64> {
        if self.connect.count == 0 {
            None
        } else if self.jitter_count == 0 {
            Some(0.0)
        } else {
            Some(self.jitter_sum / self.jitter_count as f64)
        }
    }

    /// Population standard deviation over the sample window.
    pub fn stddev_ms(&self) -> Option<f64> {
        if self.window.is_empty() {
            return None;
        }
        let n = self.window.len() as f64;
        let mean = self.window.iter().sum::<f64>() / n;
        let variance = self.window.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
        Some(variance.sqrt())
    }

    pub fn tls_min_ms(&self) -> Option<f64> {
        self.tls.min()
    }

    pub fn tls_max_ms(&self) -> Option<f64> {
        self.tls.max()
    }

    pub fn tls_avg_ms(&self) -> Option<f64> {
        self.tls.avg()
    }

    pub fn p95_ms(&self) -> Option<f64> {
        self.percentile(95.0)
    }

    pub fn p99_ms(&self) -> Option<f64> {
        self.percentile(99.0)
    }

    /// Nearest rank over the sample window.
    fn percentile(&self, p: f64) -> Option<f64> {
        if self.window.is_empty() {
            return None;
        }
        let mut values: Vec<f64> = self.window.iter().copied().collect();
        let rank = ((p / 100.0 * values.len() as f64).ceil() as usize).clamp(1, values.len());
        let (_, value, _) = values.select_nth_unstable_by(rank - 1, f64::total_cmp);
        Some(*value)
    }

    pub fn ttfb_min_ms(&self) -> Option<f64> {
        self.ttfb.min()
    }

    pub fn ttfb_max_ms(&self) -> Option<f64> {
        self.ttfb.max()
    }

    pub fn ttfb_avg_ms(&self) -> Option<f64> {
        self.ttfb.avg()
    }

    /// Connect times of the latest attempts (at most `TREND`); `None` = failed.
    pub fn recent(&self) -> &VecDeque<Option<f64>> {
        &self.recent
    }
}

/// One `Stats` per probed address, in address order.
#[derive(Debug, Clone)]
pub struct StatsSet {
    entries: Vec<(IpAddr, Stats)>,
}

impl StatsSet {
    pub fn new(addrs: &[IpAddr]) -> Self {
        StatsSet {
            entries: addrs.iter().map(|a| (*a, Stats::default())).collect(),
        }
    }

    /// Results for an address that is not in the set are ignored.
    pub fn record(&mut self, r: &ProbeResult) {
        if let Some((_, stats)) = self.entries.iter_mut().find(|(addr, _)| *addr == r.addr) {
            stats.record(r);
        }
    }

    pub fn get(&self, addr: IpAddr) -> Option<&Stats> {
        self.entries
            .iter()
            .find(|(a, _)| *a == addr)
            .map(|(_, stats)| stats)
    }

    pub fn iter(&self) -> impl Iterator<Item = (IpAddr, &Stats)> {
        self.entries.iter().map(|(addr, stats)| (*addr, stats))
    }

    pub fn attempted(&self) -> u64 {
        self.entries.iter().map(|(_, s)| s.attempted()).sum()
    }

    pub fn failed(&self) -> u64 {
        self.entries.iter().map(|(_, s)| s.failed()).sum()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::ProbeError;
    use jiff::Timestamp;
    use std::time::Duration;

    fn ok(ms: f64) -> ProbeResult {
        ProbeResult {
            seq: 0,
            at: Timestamp::UNIX_EPOCH,
            addr: std::net::Ipv4Addr::LOCALHOST.into(),
            http_status: None,
            ttfb: None,
            connect: Some(Duration::from_secs_f64(ms / 1000.0)),
            tls: None,
            error: None,
        }
    }

    fn ok_tls(ms: f64, tls_ms: f64) -> ProbeResult {
        ProbeResult {
            tls: Some(Duration::from_secs_f64(tls_ms / 1000.0)),
            ..ok(ms)
        }
    }

    fn failed() -> ProbeResult {
        ProbeResult {
            connect: None,
            error: Some(ProbeError::Timeout),
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

    fn approx(a: Option<f64>, b: f64) {
        let a = a.expect("value expected");
        assert!((a - b).abs() < 1e-6, "{a} != {b}");
    }

    #[test]
    fn empty() {
        let s = Stats::default();
        assert_eq!((s.attempted(), s.connected(), s.failed()), (0, 0, 0));
        assert_eq!(s.loss_pct(), 0.0);
        assert_eq!(s.min_ms(), None);
        assert_eq!(s.max_ms(), None);
        assert_eq!(s.avg_ms(), None);
        assert_eq!(s.median_ms(), None);
        assert_eq!(s.jitter_ms(), None);
        assert_eq!(s.stddev_ms(), None);
        assert_eq!(s.tls_avg_ms(), None);
    }

    #[test]
    fn mixed() {
        let s = stats(&[ok(10.0), failed(), ok(20.0), ok(30.0)]);
        assert_eq!((s.attempted(), s.connected(), s.failed()), (4, 3, 1));
        assert_eq!(s.loss_pct(), 25.0);
        approx(s.min_ms(), 10.0);
        approx(s.max_ms(), 30.0);
        approx(s.avg_ms(), 20.0); // not 15: failures do not count
        approx(s.median_ms(), 20.0);
        approx(s.jitter_ms(), 10.0);
        approx(s.stddev_ms(), (200.0f64 / 3.0).sqrt());
    }

    #[test]
    fn only_failures() {
        let s = stats(&[failed(), failed(), failed()]);
        assert_eq!((s.attempted(), s.connected(), s.failed()), (3, 0, 3));
        assert_eq!(s.loss_pct(), 100.0);
        assert_eq!(s.avg_ms(), None);
    }

    #[test]
    fn even_median() {
        approx(stats(&[ok(10.0), ok(20.0)]).median_ms(), 15.0);
    }

    #[test]
    fn single_sample() {
        let s = stats(&[ok(12.5)]);
        approx(s.jitter_ms(), 0.0);
        approx(s.stddev_ms(), 0.0);
        approx(s.median_ms(), 12.5);
    }

    #[test]
    fn tls_figures() {
        let s = stats(&[ok_tls(10.0, 4.0), ok_tls(12.0, 8.0), ok(11.0)]);
        approx(s.tls_min_ms(), 4.0);
        approx(s.tls_max_ms(), 8.0);
        approx(s.tls_avg_ms(), 6.0);
    }

    #[test]
    fn window_caps_samples() {
        let mut s = Stats::default();
        // 10 slow samples first, then a full window of fast ones.
        for _ in 0..10 {
            s.record(&ok(1000.0));
        }
        for _ in 0..SAMPLE_WINDOW {
            s.record(&ok(1.0));
        }
        assert_eq!(s.connected(), SAMPLE_WINDOW as u64 + 10);
        approx(s.max_ms(), 1000.0); // exact over everything
        approx(
            s.avg_ms(),
            (10.0 * 1000.0 + SAMPLE_WINDOW as f64) / (SAMPLE_WINDOW as f64 + 10.0),
        );
        approx(s.median_ms(), 1.0); // the window only
        approx(s.stddev_ms(), 0.0);
    }

    #[test]
    fn percentiles() {
        let s = stats(&(1..=100).map(|ms| ok(ms as f64)).collect::<Vec<_>>());
        approx(s.p95_ms(), 95.0);
        approx(s.p99_ms(), 99.0);
        let one = stats(&[ok(7.0)]);
        approx(one.p95_ms(), 7.0);
        approx(one.p99_ms(), 7.0);
        assert_eq!(Stats::default().p95_ms(), None);
    }

    #[test]
    fn ttfb_figures() {
        let with_ttfb = |ms: f64, ttfb_ms: f64| ProbeResult {
            http_status: Some(200),
            ttfb: Some(Duration::from_secs_f64(ttfb_ms / 1000.0)),
            ..ok(ms)
        };
        let s = stats(&[with_ttfb(1.0, 20.0), with_ttfb(1.0, 40.0), ok(1.0)]);
        approx(s.ttfb_min_ms(), 20.0);
        approx(s.ttfb_max_ms(), 40.0);
        approx(s.ttfb_avg_ms(), 30.0);
        assert_eq!(Stats::default().ttfb_avg_ms(), None);
    }

    #[test]
    fn recent_caps_at_trend_and_marks_failures() {
        let mut s = Stats::default();
        for i in 0..(TREND + 5) {
            s.record(&ok(i as f64));
        }
        s.record(&failed());
        assert_eq!(s.recent().len(), TREND);
        assert_eq!(s.recent().back(), Some(&None));
        assert_eq!(s.recent().front(), Some(&Some(6.0)));
    }

    #[test]
    fn stats_set_routes_by_addr() {
        let v4: IpAddr = "127.0.0.1".parse().unwrap();
        let v6: IpAddr = "::1".parse().unwrap();
        let at = |addr: IpAddr, r: ProbeResult| ProbeResult { addr, ..r };
        let mut set = StatsSet::new(&[v4, v6]);
        set.record(&at(v4, ok(10.0)));
        set.record(&at(v6, failed()));
        set.record(&at(v6, ok(20.0)));
        assert_eq!(set.len(), 2);
        assert_eq!(set.get(v4).unwrap().attempted(), 1);
        assert_eq!(set.get(v6).unwrap().failed(), 1);
        approx(set.get(v6).unwrap().avg_ms(), 20.0);
        assert_eq!((set.attempted(), set.failed()), (3, 1));
        let order: Vec<IpAddr> = set.iter().map(|(addr, _)| addr).collect();
        assert_eq!(order, [v4, v6]);
        assert!(set.get("10.0.0.1".parse().unwrap()).is_none());
    }
}
