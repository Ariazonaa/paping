use std::net::{IpAddr, ToSocketAddrs};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::cli::Family;

/// How long the one DNS lookup at startup may take.
pub const DNS_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// The destination as the user typed it.
    pub host: String,
    /// One or two addresses, IPv4 first.
    pub addrs: Vec<IpAddr>,
    pub is_literal: bool,
    /// `None` for IP literals, which are not looked up.
    pub dns_time: Option<Duration>,
}

impl Target {
    /// The first address: IPv4 when there is one.
    pub fn primary(&self) -> IpAddr {
        self.addrs[0]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResolveError {
    NotFound,
    NoAddressForFamily,
    Timeout,
}

/// Resolves `host` once, like the original. IP literals are used as they are. A name
/// yields the first IPv4 and the first IPv6 address it has (`Family::Any`), or the first
/// address of the requested family. The lookup runs on its own thread so a hanging
/// resolver cannot block past `timeout`.
pub fn resolve(host: &str, family: Family, timeout: Duration) -> Result<Target, ResolveError> {
    if let Ok(addr) = host.parse::<IpAddr>() {
        return if matches_family(addr, family) {
            Ok(Target {
                host: host.to_string(),
                addrs: vec![addr],
                is_literal: true,
                dns_time: None,
            })
        } else {
            Err(ResolveError::NoAddressForFamily)
        };
    }

    let (tx, rx) = mpsc::channel();
    let name = host.to_string();
    thread::spawn(move || {
        let started = Instant::now();
        let result = (name.as_str(), 0)
            .to_socket_addrs()
            .map(|addrs| addrs.map(|a| a.ip()).collect::<Vec<_>>());
        let _ = tx.send((result, started.elapsed()));
    });

    let (result, elapsed) = rx
        .recv_timeout(timeout)
        .map_err(|_| ResolveError::Timeout)?;
    let addrs = result.map_err(|_| ResolveError::NotFound)?;
    if addrs.is_empty() {
        return Err(ResolveError::NotFound);
    }
    let addrs = pick(&addrs, family);
    if addrs.is_empty() {
        return Err(ResolveError::NoAddressForFamily);
    }
    Ok(Target {
        host: host.to_string(),
        addrs,
        is_literal: false,
        dns_time: Some(elapsed),
    })
}

/// The addresses paping probes: the first of each family that `family` allows, IPv4 first.
fn pick(addrs: &[IpAddr], family: Family) -> Vec<IpAddr> {
    let v4 = addrs.iter().copied().find(IpAddr::is_ipv4);
    let v6 = addrs.iter().copied().find(IpAddr::is_ipv6);
    [v4, v6]
        .into_iter()
        .flatten()
        .filter(|a| matches_family(*a, family))
        .collect()
}

fn matches_family(addr: IpAddr, family: Family) -> bool {
    match family {
        Family::Any => true,
        Family::V4 => addr.is_ipv4(),
        Family::V6 => addr.is_ipv6(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn literal_v4() {
        let t = resolve("127.0.0.1", Family::Any, DNS_TIMEOUT).unwrap();
        assert_eq!(t.addrs, [ip("127.0.0.1")]);
        assert_eq!(t.primary(), ip("127.0.0.1"));
        assert!(t.is_literal);
        assert_eq!(t.dns_time, None);
        assert_eq!(t.host, "127.0.0.1");
    }

    #[test]
    fn literal_v6() {
        let t = resolve("::1", Family::Any, DNS_TIMEOUT).unwrap();
        assert_eq!(t.addrs, [ip("::1")]);
        assert!(t.is_literal);
    }

    #[test]
    fn literal_family_mismatch() {
        assert_eq!(
            resolve("127.0.0.1", Family::V6, DNS_TIMEOUT),
            Err(ResolveError::NoAddressForFamily)
        );
        assert_eq!(
            resolve("::1", Family::V4, DNS_TIMEOUT),
            Err(ResolveError::NoAddressForFamily)
        );
    }

    #[test]
    fn localhost_v4_gives_one() {
        let t = resolve("localhost", Family::V4, DNS_TIMEOUT).unwrap();
        assert_eq!(t.addrs, [ip("127.0.0.1")]);
        assert!(!t.is_literal);
        assert!(t.dns_time.is_some());
        assert_eq!(t.host, "localhost");
    }

    #[test]
    fn localhost_any_gives_both_families() {
        let system: Vec<IpAddr> = ("localhost", 0)
            .to_socket_addrs()
            .unwrap()
            .map(|a| a.ip())
            .collect();
        let t = resolve("localhost", Family::Any, DNS_TIMEOUT).unwrap();
        if system.iter().any(IpAddr::is_ipv4) && system.iter().any(IpAddr::is_ipv6) {
            assert_eq!(t.addrs, [ip("127.0.0.1"), ip("::1")]);
        } else {
            assert_eq!(t.addrs.len(), 1);
        }
    }

    #[test]
    fn v4_first_even_if_system_prefers_v6() {
        assert_eq!(
            pick(&[ip("::1"), ip("127.0.0.1")], Family::Any),
            [ip("127.0.0.1"), ip("::1")]
        );
        assert_eq!(
            pick(
                &[
                    ip("2001:db8::1"),
                    ip("2001:db8::2"),
                    ip("10.0.0.1"),
                    ip("10.0.0.2")
                ],
                Family::Any
            ),
            [ip("10.0.0.1"), ip("2001:db8::1")]
        );
        assert_eq!(pick(&[ip("::1"), ip("127.0.0.1")], Family::V6), [ip("::1")]);
        assert_eq!(pick(&[ip("::1")], Family::Any), [ip("::1")]);
        assert_eq!(pick(&[ip("::1")], Family::V4), Vec::<IpAddr>::new());
    }

    #[test]
    fn unknown_host_is_an_error() {
        // `.invalid` never resolves, but the Windows resolver takes about 11 s to say so,
        // so this also exercises the timeout path there. Both end as "Cannot resolve host".
        let started = Instant::now();
        let result = resolve(
            "does-not-exist.invalid",
            Family::Any,
            Duration::from_secs(1),
        );
        assert!(
            matches!(result, Err(ResolveError::NotFound | ResolveError::Timeout)),
            "{result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
