# Changelog

## [0.1.0] - 2026-10-27

First release of the Rust rewrite. Command line and text output are compatible with
paping 1.5.5.

### Added

- IPv6. A name with IPv4 and IPv6 addresses is checked over both at the same time, with
  statistics per address; `-4` / `-6` pick one family.
- `host:port`, `[ipv6]:port` and service names such as `https` or `ssh` for the port.
- `--http` (and `--http-path`) sends a GET and shows the status code and the time to the
  first byte; HTTPS on port 443 automatically.
- `--tls` measures a TLS handshake after each connect.
- `--details` shows the DNS lookup time, median, jitter, standard deviation, and TLS/HTTP
  times.
- `--interval` to set the time between rounds.
- On a terminal: a live status line with a sparkline, colors by latency, and a trend line in
  the statistics.
- `--json` (NDJSON) and `--csv` output for scripts, including p95/p99 per address.
- `--tui`, a live full-screen view: big digits for the latest value, one chart line per
  address family with gaps at failures, percentiles, a histogram and a compact log.
- `-V`, `--version`.

### Changed

- 1.5.5 was IPv4-only; now both families are checked when a name has both. `-4` restores
  the old behaviour. `--ip-bind` picks the family of its address.
- A closed port reports `Connection refused`, and unreachable networks or hosts get their
  own messages. 1.5.5 reported these as a timeout or even as a success.
- `Average` is the mean of successful attempts only. 1.5.5 divided by all attempts.
- The header shows the name you typed and its addresses instead of the canonical (CNAME)
  name.
- Invalid numbers such as `-p abc` are a usage error instead of being read as 0.
- Exit code 2 for usage errors and unresolvable hosts.
- Colors turn off when stdout is not a terminal or `NO_COLOR` is set.
- On Windows, a closed port is reported at once instead of after about 2 s of retries.
