use std::io;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use jiff::Timestamp;
use socket2::{Domain, Protocol, Socket, Type};

use crate::http::{self, HttpSetup};
use crate::tls::{self, TlsSetup};

#[derive(Debug, Clone, PartialEq)]
pub enum ProbeError {
    Timeout,
    Refused,
    NetUnreachable,
    HostUnreachable,
    Tls(String),
    /// No valid HTTP answer (`--http`).
    Http(String),
    /// An HTTP answer with status 500 or higher.
    HttpStatus(u16),
    Other(String),
}

impl ProbeError {
    /// Stable code for JSON and CSV.
    pub fn code(&self) -> &'static str {
        match self {
            ProbeError::Timeout => "timeout",
            ProbeError::Refused => "refused",
            ProbeError::NetUnreachable => "net_unreachable",
            ProbeError::HostUnreachable => "host_unreachable",
            ProbeError::Tls(_) => "tls",
            ProbeError::Http(_) => "http",
            ProbeError::HttpStatus(_) => "http_status",
            ProbeError::Other(_) => "other",
        }
    }

    /// Free-text detail, only for errors whose code does not say everything.
    pub fn detail(&self) -> Option<String> {
        match self {
            ProbeError::Tls(detail) | ProbeError::Http(detail) | ProbeError::Other(detail) => {
                Some(detail.clone())
            }
            ProbeError::HttpStatus(status) => Some(status.to_string()),
            _ => None,
        }
    }

    /// The line the text output prints for a failed attempt.
    pub fn message(&self) -> String {
        match self {
            ProbeError::Timeout => "Connection timed out".into(),
            ProbeError::Refused => "Connection refused".into(),
            ProbeError::NetUnreachable => "Network unreachable".into(),
            ProbeError::HostUnreachable => "Host unreachable".into(),
            ProbeError::Tls(detail) => format!("TLS handshake failed ({detail})"),
            ProbeError::Http(detail) => format!("HTTP request failed ({detail})"),
            ProbeError::HttpStatus(status) => format!("HTTP {status}"),
            // As in the original; the detail only goes to JSON and CSV.
            ProbeError::Other(_) => "General failure".into(),
        }
    }

    fn from_io(e: &io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => ProbeError::Timeout,
            io::ErrorKind::ConnectionRefused => ProbeError::Refused,
            io::ErrorKind::NetworkUnreachable => ProbeError::NetUnreachable,
            io::ErrorKind::HostUnreachable => ProbeError::HostUnreachable,
            _ => ProbeError::Other(e.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub seq: u64,
    /// The address this attempt went to.
    pub addr: IpAddr,
    /// When the attempt started.
    pub at: Timestamp,
    pub connect: Option<Duration>,
    pub tls: Option<Duration>,
    pub http_status: Option<u16>,
    /// Time from the sent HTTP request to the first byte of the answer.
    pub ttfb: Option<Duration>,
    pub error: Option<ProbeError>,
}

impl ProbeResult {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }
}

#[derive(Debug, Clone)]
pub struct ProbeSpec {
    pub addr: SocketAddr,
    pub bind: Option<IpAddr>,
    pub timeout: Duration,
    /// With `Some`, a TLS handshake follows every successful connect.
    pub tls: Option<TlsSetup>,
    /// With `Some`, an HTTP request follows the connect (and the TLS handshake).
    pub http: Option<HttpSetup>,
}

/// One attempt: optional bind, then a connect bounded by `spec.timeout`, then the optional
/// TLS handshake, bounded by the same timeout.
pub fn probe(spec: &ProbeSpec, seq: u64) -> ProbeResult {
    let mut result = ProbeResult {
        seq,
        addr: spec.addr.ip(),
        at: Timestamp::now(),
        connect: None,
        tls: None,
        http_status: None,
        ttfb: None,
        error: None,
    };
    let socket = match connect(spec) {
        Ok((socket, elapsed)) => {
            result.connect = Some(elapsed);
            socket
        }
        Err(e) => {
            result.error = Some(e);
            return result;
        }
    };
    let stream = TcpStream::from(socket);
    match &spec.tls {
        Some(setup) => match tls::handshake(stream, setup, spec.timeout) {
            Ok((elapsed, mut tls_stream)) => {
                result.tls = Some(elapsed);
                if let Some(http) = &spec.http {
                    let outcome = http::request(&mut tls_stream, http, spec.timeout);
                    record_http(&mut result, outcome);
                }
            }
            Err(detail) => result.error = Some(ProbeError::Tls(detail)),
        },
        None => {
            if let Some(http) = &spec.http {
                let mut stream = stream;
                let outcome = http::request(&mut stream, http, spec.timeout);
                record_http(&mut result, outcome);
            }
        }
    }
    result
}

/// Any HTTP answer below 500 means the service answers; 5xx counts as a failure.
fn record_http(result: &mut ProbeResult, outcome: Result<(u16, Duration), String>) {
    match outcome {
        Ok((status, ttfb)) => {
            result.http_status = Some(status);
            result.ttfb = Some(ttfb);
            if status >= 500 {
                result.error = Some(ProbeError::HttpStatus(status));
            }
        }
        Err(detail) => result.error = Some(ProbeError::Http(detail)),
    }
}

fn connect(spec: &ProbeSpec) -> Result<(Socket, Duration), ProbeError> {
    let socket = Socket::new(
        Domain::for_address(spec.addr),
        Type::STREAM,
        Some(Protocol::TCP),
    )
    .map_err(|e| ProbeError::Other(e.to_string()))?;
    if let Some(bind) = spec.bind {
        socket
            .bind(&SocketAddr::new(bind, 0).into())
            .map_err(|e| ProbeError::Other(e.to_string()))?;
    }
    #[cfg(windows)]
    send_single_syn(&socket, spec.timeout);
    let started = Instant::now();
    socket
        .connect_timeout(&spec.addr.into(), spec.timeout)
        .map_err(|e| ProbeError::from_io(&e))?;
    Ok((socket, started.elapsed()))
}

/// Windows answers an RST to its SYN by retrying for about 2 s, so a closed port shows up
/// as a timeout at the default `-t 1000`. Sending exactly one SYN whose retransmission
/// timer equals the probe timeout makes "refused" immediate and still waits the full
/// timeout for a slow answer. Older Windows without this ioctl keeps the default behaviour.
#[cfg(windows)]
fn send_single_syn(socket: &Socket, timeout: Duration) {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
        SIO_TCP_INITIAL_RTO, TCP_INITIAL_RTO_PARAMETERS, WSAIoctl,
    };
    const NO_SYN_RETRANSMISSIONS: u8 = 0xFE;

    let params = TCP_INITIAL_RTO_PARAMETERS {
        Rtt: timeout.as_millis().clamp(1, u16::MAX as u128) as u16,
        MaxSynRetransmissions: NO_SYN_RETRANSMISSIONS,
    };
    let mut returned = 0u32;
    // SAFETY: the socket is open, the input buffer is a live TCP_INITIAL_RTO_PARAMETERS of
    // the given size, and no output buffer or overlapped I/O is used.
    unsafe {
        WSAIoctl(
            socket.as_raw_socket() as usize,
            SIO_TCP_INITIAL_RTO,
            (&raw const params).cast(),
            size_of::<TCP_INITIAL_RTO_PARAMETERS>() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    /// A listener on 127.0.0.1 that accepts and drops connections in the background.
    fn listener() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        thread::spawn(move || for _ in l.incoming() {});
        addr
    }

    fn closed_port() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    }

    fn spec(addr: SocketAddr, timeout_ms: u64) -> ProbeSpec {
        ProbeSpec {
            addr,
            bind: None,
            timeout: Duration::from_millis(timeout_ms),
            tls: None,
            http: None,
        }
    }

    #[test]
    fn connects_to_listener() {
        let r = probe(&spec(listener(), 1000), 7);
        assert!(r.ok(), "{:?}", r.error);
        assert_eq!(r.seq, 7);
        assert_eq!(r.addr, "127.0.0.1".parse::<IpAddr>().unwrap());
        assert_eq!((r.http_status, r.ttfb), (None, None));
        assert!(r.connect.unwrap() < Duration::from_millis(1000));
        assert_eq!(r.tls, None);
    }

    #[test]
    fn refused_on_closed_port() {
        let r = probe(&spec(closed_port(), 3000), 1);
        assert_eq!(r.error, Some(ProbeError::Refused));
        assert_eq!(r.connect, None);
        assert!(!r.ok());
    }

    #[test]
    fn refused_at_default_timeout() {
        let r = probe(&spec(closed_port(), 1000), 1);
        assert_eq!(r.error, Some(ProbeError::Refused));
    }

    #[test]
    fn binds_source_address() {
        let mut s = spec(listener(), 1000);
        s.bind = Some("127.0.0.1".parse().unwrap());
        let r = probe(&s, 1);
        assert!(r.ok(), "{:?}", r.error);

        s.bind = Some("::1".parse().unwrap());
        let r = probe(&s, 2);
        assert!(
            matches!(r.error, Some(ProbeError::Other(_))),
            "{:?}",
            r.error
        );
    }

    #[test]
    fn connects_v6() {
        let Ok(l) = TcpListener::bind("[::1]:0") else {
            return; // no IPv6 loopback on this machine
        };
        let addr = l.local_addr().unwrap();
        thread::spawn(move || for _ in l.incoming() {});
        let r = probe(&spec(addr, 1000), 1);
        assert!(r.ok(), "{:?}", r.error);
    }

    #[test]
    #[ignore = "needs a network that drops packets to 10.255.255.1"]
    fn timeout_on_blackhole() {
        let r = probe(&spec("10.255.255.1:80".parse().unwrap(), 300), 1);
        assert_eq!(r.error, Some(ProbeError::Timeout));
    }

    #[test]
    #[ignore = "needs a network that drops packets to 10.255.255.1"]
    fn blackhole_waits_the_full_timeout() {
        let started = Instant::now();
        let r = probe(&spec("10.255.255.1:80".parse().unwrap(), 2500), 1);
        assert_eq!(r.error, Some(ProbeError::Timeout));
        assert!(
            started.elapsed() >= Duration::from_millis(2400),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn error_codes_and_messages() {
        let cases = [
            (ProbeError::Timeout, "timeout", None, "Connection timed out"),
            (ProbeError::Refused, "refused", None, "Connection refused"),
            (
                ProbeError::NetUnreachable,
                "net_unreachable",
                None,
                "Network unreachable",
            ),
            (
                ProbeError::HostUnreachable,
                "host_unreachable",
                None,
                "Host unreachable",
            ),
            (
                ProbeError::Tls("bad record".into()),
                "tls",
                Some("bad record"),
                "TLS handshake failed (bad record)",
            ),
            (
                ProbeError::Http("timed out".into()),
                "http",
                Some("timed out"),
                "HTTP request failed (timed out)",
            ),
            (
                ProbeError::HttpStatus(503),
                "http_status",
                Some("503"),
                "HTTP 503",
            ),
            (
                ProbeError::Other("boom".into()),
                "other",
                Some("boom"),
                "General failure",
            ),
        ];
        for (err, code, detail, message) in cases {
            assert_eq!(err.code(), code);
            assert_eq!(err.detail().as_deref(), detail);
            assert_eq!(err.message(), message);
        }
    }
}
