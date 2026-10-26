use std::ffi::OsString;
use std::net::IpAddr;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Family {
    Any,
    V4,
    V6,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputMode {
    Text,
    Json,
    Csv,
    Tui,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub destination: String,
    pub port: u16,
    pub timeout: Duration,
    /// `None` = probe until interrupted.
    pub count: Option<u64>,
    /// `false` only via `--nocolor`; terminal and `NO_COLOR` checks happen in the output layer.
    pub color: bool,
    pub bind: Option<IpAddr>,
    pub family: Family,
    pub interval: Duration,
    pub tls: bool,
    pub details: bool,
    pub output: OutputMode,
    /// `--http`: send a GET after connecting and measure the time to the first byte.
    pub http: bool,
    pub http_path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Run(Config),
    Help,
    Version,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CliError(pub String);

/// The original 1.5.5 usage text, followed by the options this rewrite adds.
pub const USAGE: &str = "Syntax: paping [options] destination

Options:
 -?, --help\tdisplay usage
 -p, --port N\tset TCP port N (required)
     --nocolor\tDisable color output
 -t, --timeout\ttimeout in milliseconds (default 1000)
 -c, --count N\tset number of checks to N
     --ip-bind\tset source ip address

Extended options:
 -4, -6\t\tuse only IPv4 / IPv6 addresses
     --interval N\tdelay between checks in milliseconds (default 1000)
     --tls\t\tmeasure a TLS handshake after connecting
     --details\tshow DNS time, median, jitter and standard deviation
     --json\toutput NDJSON
     --csv\t\toutput CSV
     --tui\t\tlive full-screen view
 -V, --version\tshow version
     --http\t\tsend GET and show status and time to first byte
     --http-path P\tpath for --http (default /)

destination may be host:port or [ipv6]:port; -p accepts names like https
";

/// Well-known port names accepted by `-p` and in `host:port`.
pub fn service_port(name: &str) -> Option<u16> {
    Some(match name {
        "http" => 80,
        "https" => 443,
        "ssh" => 22,
        "ftp" => 21,
        "smtp" => 25,
        "dns" => 53,
        "imap" => 143,
        "imaps" => 993,
        "pop3" => 110,
        "pop3s" => 995,
        "submission" => 587,
        "rdp" => 3389,
        "mysql" => 3306,
        "postgres" => 5432,
        "redis" => 6379,
        _ => return None,
    })
}

/// A port number (1–65535) or a service name.
fn parse_port(raw: &str) -> Result<u16, CliError> {
    match raw.parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        Ok(_) => Err(CliError(format!("invalid port: {raw}"))),
        Err(_) => service_port(raw).ok_or_else(|| CliError(format!("invalid port: {raw}"))),
    }
}

/// Splits `host:port` and `[ipv6]:port`. A bare IPv6 literal (`::1`, `2001:db8::1:80`) has
/// no port: with more than one `:` and no brackets, everything is the address.
fn split_destination(raw: &str) -> Result<(String, Option<String>), CliError> {
    if raw.parse::<IpAddr>().is_ok() {
        return Ok((raw.to_string(), None));
    }
    if let Some(rest) = raw.strip_prefix('[') {
        let invalid = || CliError(format!("invalid destination: {raw}"));
        let (host, after) = rest.split_once(']').ok_or_else(invalid)?;
        return match after {
            "" => Ok((host.to_string(), None)),
            _ => match after.strip_prefix(':') {
                Some(port) => Ok((host.to_string(), Some(port.to_string()))),
                None => Err(invalid()),
            },
        };
    }
    match raw.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => Ok((host.to_string(), Some(port.to_string()))),
        _ => Ok((raw.to_string(), None)),
    }
}

pub fn banner() -> String {
    format!("paping v{}", env!("CARGO_PKG_VERSION"))
}

/// Parses the arguments after `argv[0]`.
///
/// Like the original, flags and the destination may come in any order and a repeated
/// flag overrides the earlier value. `--help` and `--version` win over any other error.
pub fn parse<I: IntoIterator<Item = OsString>>(args: I) -> Result<Command, CliError> {
    use lexopt::Arg::{Long, Short, Value};

    let mut parser = lexopt::Parser::from_args(args);
    let mut first_err: Option<String> = None;
    let mut fail = |msg: String| {
        first_err.get_or_insert(msg);
    };

    let mut destination: Option<String> = None;
    let mut port: Option<String> = None;
    let mut timeout: Option<String> = None;
    let mut count: Option<String> = None;
    let mut interval: Option<String> = None;
    let mut bind: Option<String> = None;
    let mut color = true;
    let (mut v4, mut v6) = (false, false);
    let (mut tls, mut details) = (false, false);
    let mut outputs: Vec<OutputMode> = Vec::new();
    let mut http = false;
    let mut http_path: Option<String> = None;

    loop {
        let arg = match parser.next() {
            Ok(Some(arg)) => arg,
            Ok(None) => break,
            Err(e) => {
                fail(e.to_string());
                continue;
            }
        };
        let slot = match arg {
            Short('?') | Long("help") => return Ok(Command::Help),
            Short('V') | Long("version") => return Ok(Command::Version),
            Short('p') | Long("port") => &mut port,
            Short('t') | Long("timeout") => &mut timeout,
            Short('c') | Long("count") => &mut count,
            Long("interval") => &mut interval,
            Long("ip-bind") => &mut bind,
            Long("http-path") => &mut http_path,
            Long("http") => {
                http = true;
                continue;
            }
            Long("nocolor") => {
                color = false;
                continue;
            }
            Short('4') => {
                v4 = true;
                continue;
            }
            Short('6') => {
                v6 = true;
                continue;
            }
            Long("tls") => {
                tls = true;
                continue;
            }
            Long("details") => {
                details = true;
                continue;
            }
            Long("json") | Long("csv") | Long("tui") => {
                outputs.push(match arg {
                    Long("json") => OutputMode::Json,
                    Long("csv") => OutputMode::Csv,
                    _ => OutputMode::Tui,
                });
                continue;
            }
            Value(value) => {
                let value = value.to_string_lossy().into_owned();
                if destination.is_none() {
                    destination = Some(value);
                } else {
                    fail(format!("unexpected argument: {value}"));
                }
                continue;
            }
            Short(c) => {
                fail(format!("unknown option: -{c}"));
                continue;
            }
            Long(name) => {
                fail(format!("unknown option: --{name}"));
                continue;
            }
        };
        let flag = match arg {
            Short(c) => format!("-{c}"),
            Long(name) => format!("--{name}"),
            Value(_) => unreachable!(),
        };
        match parser.value() {
            Ok(value) => *slot = Some(value.to_string_lossy().into_owned()),
            Err(_) => fail(format!("missing value for {flag}")),
        }
    }

    if let Some(msg) = first_err {
        return Err(CliError(msg));
    }

    let destination = destination.ok_or_else(|| CliError("destination is required".into()))?;
    let (destination, dest_port) = split_destination(&destination)?;
    let port = match (dest_port, port) {
        (None, None) => return Err(CliError("--port is required".into())),
        (Some(raw), None) | (None, Some(raw)) => parse_port(&raw)?,
        (Some(in_dest), Some(flag)) => {
            let port = parse_port(&in_dest)?;
            if parse_port(&flag)? != port {
                return Err(CliError(format!("port given twice: {in_dest} and {flag}")));
            }
            port
        }
    };
    let http_path = match http_path {
        None => "/".to_string(),
        Some(path) if path.starts_with('/') => path,
        Some(path) => return Err(CliError(format!("invalid --http-path: {path}"))),
    };
    // HTTP on 443 means HTTPS; elsewhere `--tls` asks for it.
    let tls = tls || (http && port == 443);
    let timeout = match timeout {
        None => 1000,
        Some(raw) => match raw.parse::<i64>() {
            Ok(t) if t > 0 => t as u64,
            _ => return Err(CliError(format!("invalid timeout: {raw}"))),
        },
    };
    let count = match count {
        None => None,
        Some(raw) => match raw.parse::<i64>() {
            Ok(c) if c > 0 => Some(c as u64),
            Ok(_) => None,
            Err(_) => return Err(CliError(format!("invalid count: {raw}"))),
        },
    };
    let interval = match interval {
        None => 1000,
        Some(raw) => raw
            .parse::<u64>()
            .map_err(|_| CliError(format!("invalid interval: {raw}")))?,
    };
    let bind = match bind {
        None => None,
        Some(raw) => Some(
            raw.parse::<IpAddr>()
                .map_err(|_| CliError(format!("invalid address: {raw}")))?,
        ),
    };
    let family = match (v4, v6) {
        (true, true) => return Err(CliError("-4 and -6 are mutually exclusive".into())),
        (true, false) => Family::V4,
        (false, true) => Family::V6,
        (false, false) => Family::Any,
    };
    // A source address only works toward the same family, so it also picks the
    // destination address. 1.5.5 was IPv4-only, so `--ip-bind <IPv4>` always matched.
    let family = match (family, bind) {
        (Family::Any, Some(ip)) if ip.is_ipv4() => Family::V4,
        (Family::Any, Some(_)) => Family::V6,
        (Family::V4, Some(ip)) if ip.is_ipv6() => {
            return Err(CliError(format!("--ip-bind {ip} does not match -4")));
        }
        (Family::V6, Some(ip)) if ip.is_ipv4() => {
            return Err(CliError(format!("--ip-bind {ip} does not match -6")));
        }
        (family, _) => family,
    };
    outputs.dedup();
    let output = match outputs.as_slice() {
        [] => OutputMode::Text,
        [one] => *one,
        _ => {
            return Err(CliError(
                "--json, --csv and --tui are mutually exclusive".into(),
            ));
        }
    };

    Ok(Command::Run(Config {
        destination,
        port,
        timeout: Duration::from_millis(timeout),
        count,
        color,
        bind,
        family,
        interval: Duration::from_millis(interval),
        tls,
        details,
        output,
        http,
        http_path,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Command, CliError> {
        parse(args.iter().map(OsString::from))
    }

    fn run(args: &[&str]) -> Config {
        match p(args) {
            Ok(Command::Run(cfg)) => cfg,
            other => panic!("expected Run, got {other:?}"),
        }
    }

    fn err(args: &[&str]) -> String {
        match p(args) {
            Err(CliError(msg)) => msg,
            other => panic!("expected error, got {other:?}"),
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn minimal() {
        assert_eq!(
            run(&["example.com", "-p", "443"]),
            Config {
                destination: "example.com".into(),
                port: 443,
                timeout: ms(1000),
                count: None,
                color: true,
                bind: None,
                family: Family::Any,
                interval: ms(1000),
                tls: false,
                details: false,
                output: OutputMode::Text,
                http: false,
                http_path: "/".into(),
            }
        );
    }

    #[test]
    fn host_port() {
        let cfg = run(&["example.com:443"]);
        assert_eq!((cfg.destination.as_str(), cfg.port), ("example.com", 443));
    }

    #[test]
    fn bracket_v6_port() {
        let cfg = run(&["[::1]:22"]);
        assert_eq!((cfg.destination.as_str(), cfg.port), ("::1", 22));
        assert_eq!(err(&["[::1]"]), "--port is required");
    }

    #[test]
    fn v6_literal_no_port() {
        let cfg = run(&["::1", "-p", "22"]);
        assert_eq!((cfg.destination.as_str(), cfg.port), ("::1", 22));
        let cfg = run(&["2001:db8::1:80", "-p", "1"]);
        assert_eq!((cfg.destination.as_str(), cfg.port), ("2001:db8::1:80", 1));
    }

    #[test]
    fn service_names() {
        assert_eq!(run(&["h", "-p", "https"]).port, 443);
        assert_eq!(run(&["h:ssh"]).port, 22);
        assert_eq!(run(&["h", "--port", "postgres"]).port, 5432);
        assert_eq!(err(&["h", "-p", "nope"]), "invalid port: nope");
        assert_eq!(err(&["h:nope"]), "invalid port: nope");
        assert_eq!(service_port("imaps"), Some(993));
        assert_eq!(service_port("HTTP"), None);
    }

    #[test]
    fn port_twice() {
        assert_eq!(err(&["h:443", "-p", "80"]), "port given twice: 443 and 80");
        assert_eq!(run(&["h:443", "-p", "443"]).port, 443);
        assert_eq!(run(&["h:https", "-p", "443"]).port, 443);
    }

    #[test]
    fn http_flags() {
        let cfg = run(&["h", "-p", "443", "--http"]);
        assert!(cfg.http && cfg.tls, "--http on 443 turns TLS on");
        assert_eq!(cfg.http_path, "/");
        let cfg = run(&["h", "-p", "80", "--http"]);
        assert!(cfg.http && !cfg.tls);
        let cfg = run(&[
            "h",
            "-p",
            "8443",
            "--http",
            "--tls",
            "--http-path",
            "/health",
        ]);
        assert!(cfg.http && cfg.tls);
        assert_eq!(cfg.http_path, "/health");
        assert_eq!(
            err(&["h", "-p", "80", "--http", "--http-path", "health"]),
            "invalid --http-path: health"
        );
        assert!(!run(&["h", "-p", "80", "--http-path", "/x"]).http);
    }

    #[test]
    fn original_flags_any_order() {
        let cfg = run(&["-c", "4", "host", "--port", "80", "-t", "500", "--nocolor"]);
        assert_eq!(cfg.destination, "host");
        assert_eq!(cfg.count, Some(4));
        assert_eq!(cfg.port, 80);
        assert_eq!(cfg.timeout, ms(500));
        assert!(!cfg.color);
    }

    #[test]
    fn long_forms_of_original_flags() {
        let cfg = run(&["host", "--port", "80", "--timeout", "250", "--count", "2"]);
        assert_eq!((cfg.port, cfg.timeout, cfg.count), (80, ms(250), Some(2)));
    }

    #[test]
    fn count_zero_or_negative_is_endless() {
        assert_eq!(run(&["h", "-p", "1", "-c", "0"]).count, None);
        assert_eq!(run(&["h", "-p", "1", "-c", "-3"]).count, None);
    }

    #[test]
    fn last_value_wins() {
        assert_eq!(run(&["h", "-p", "1", "-p", "2"]).port, 2);
    }

    #[test]
    fn ip_bind() {
        let v4 = run(&["h", "-p", "1", "--ip-bind", "10.0.0.5"]);
        assert_eq!(v4.bind, Some("10.0.0.5".parse().unwrap()));
        let v6 = run(&["h", "-p", "1", "--ip-bind", "::1"]);
        assert_eq!(v6.bind, Some("::1".parse().unwrap()));
    }

    #[test]
    fn ip_bind_picks_the_address_family() {
        // A host name that resolves to IPv6 first must still work with an IPv4 source.
        assert_eq!(
            run(&["h", "-p", "1", "--ip-bind", "10.0.0.5"]).family,
            Family::V4
        );
        assert_eq!(
            run(&["h", "-p", "1", "--ip-bind", "::1"]).family,
            Family::V6
        );
        assert_eq!(
            run(&["h", "-p", "1", "-4", "--ip-bind", "10.0.0.5"]).family,
            Family::V4
        );
        assert_eq!(
            err(&["h", "-p", "1", "-6", "--ip-bind", "10.0.0.5"]),
            "--ip-bind 10.0.0.5 does not match -6"
        );
        assert_eq!(
            err(&["h", "-p", "1", "-4", "--ip-bind", "::1"]),
            "--ip-bind ::1 does not match -4"
        );
    }

    #[test]
    fn extended_flags() {
        let cfg = run(&[
            "h",
            "-p",
            "1",
            "-6",
            "--interval",
            "250",
            "--tls",
            "--details",
            "--json",
        ]);
        assert_eq!(cfg.family, Family::V6);
        assert_eq!(cfg.interval, ms(250));
        assert!(cfg.tls);
        assert!(cfg.details);
        assert_eq!(cfg.output, OutputMode::Json);
        assert_eq!(run(&["h", "-p", "1", "-4", "--csv"]).family, Family::V4);
        assert_eq!(run(&["h", "-p", "1", "--csv"]).output, OutputMode::Csv);
        assert_eq!(run(&["h", "-p", "1", "--tui"]).output, OutputMode::Tui);
        assert_eq!(run(&["h", "-p", "1", "--interval", "0"]).interval, ms(0));
    }

    #[test]
    fn help_and_version() {
        assert_eq!(p(&["-?"]), Ok(Command::Help));
        assert_eq!(p(&["--help"]), Ok(Command::Help));
        assert_eq!(p(&["-V"]), Ok(Command::Version));
        assert_eq!(p(&["--version"]), Ok(Command::Version));
        assert_eq!(p(&["h", "-p", "x", "--help"]), Ok(Command::Help));
    }

    #[test]
    fn errors() {
        assert_eq!(err(&["h"]), "--port is required");
        assert_eq!(err(&["h", "-p", "0"]), "invalid port: 0");
        assert_eq!(err(&["h", "-p", "70000"]), "invalid port: 70000");
        assert_eq!(err(&["h", "-p", "abc"]), "invalid port: abc");
        assert_eq!(err(&["h", "-p", "1", "-t", "0"]), "invalid timeout: 0");
        assert_eq!(err(&["h", "-p", "1", "-t", "x"]), "invalid timeout: x");
        assert_eq!(err(&["h", "-p", "1", "-c", "x"]), "invalid count: x");
        assert_eq!(
            err(&["h", "-p", "1", "--interval", "-1"]),
            "invalid interval: -1"
        );
        assert_eq!(
            err(&["h", "-p", "1", "--ip-bind", "1.2.3"]),
            "invalid address: 1.2.3"
        );
        assert_eq!(err(&["-p", "1"]), "destination is required");
        assert_eq!(err(&[]), "destination is required");
        assert_eq!(err(&["a", "b", "-p", "1"]), "unexpected argument: b");
        assert_eq!(
            err(&["h", "-p", "1", "--json", "--csv"]),
            "--json, --csv and --tui are mutually exclusive"
        );
        assert_eq!(
            err(&["h", "-p", "1", "-4", "-6"]),
            "-4 and -6 are mutually exclusive"
        );
        assert_eq!(err(&["h", "-p", "1", "--foo"]), "unknown option: --foo");
        assert_eq!(err(&["h", "-p"]), "missing value for -p");
        assert_eq!(err(&["h", "--port"]), "missing value for --port");
    }

    #[test]
    fn usage_text_is_original_plus_extended() {
        assert!(USAGE.starts_with(
            "Syntax: paping [options] destination\n\nOptions:\n -?, --help\tdisplay usage\n -p, --port N\tset TCP port N (required)\n     --nocolor\tDisable color output\n -t, --timeout\ttimeout in milliseconds (default 1000)\n -c, --count N\tset number of checks to N\n     --ip-bind\tset source ip address\n\nExtended options:\n"
        ));
        for flag in [
            "--interval",
            "--tls",
            "--details",
            "--json",
            "--csv",
            "--tui",
            "--version",
        ] {
            assert!(USAGE.contains(flag), "{flag} missing from usage");
        }
    }

    #[test]
    fn banner_has_version() {
        assert_eq!(banner(), format!("paping v{}", env!("CARGO_PKG_VERSION")));
    }
}
