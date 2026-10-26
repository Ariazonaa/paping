//! `--http`: one GET after connecting; only the status line is read.

use std::io::{self, Read, Write};
use std::net::{IpAddr, TcpStream};

use crate::tls::TlsStream;
use std::time::{Duration, Instant};

/// What a probe needs to send its request.
#[derive(Debug, Clone)]
pub struct HttpSetup {
    /// Sent as the `Host` header.
    pub host: String,
    pub path: String,
}

/// A stream whose underlying TCP socket can take read and write timeouts.
pub trait TimedStream: Read + Write {
    fn set_timeouts(&self, timeout: Duration) -> io::Result<()>;
}

impl TimedStream for TcpStream {
    fn set_timeouts(&self, timeout: Duration) -> io::Result<()> {
        self.set_read_timeout(Some(timeout))?;
        self.set_write_timeout(Some(timeout))
    }
}

impl TimedStream for TlsStream {
    fn set_timeouts(&self, timeout: Duration) -> io::Result<()> {
        self.sock.set_timeouts(timeout)
    }
}

/// Sends the request and returns the status code and the time from the sent request to
/// the first byte of the answer. The whole step ends after `timeout`: before every write
/// and read, the socket gets the time that is left.
pub fn request<S: TimedStream>(
    stream: &mut S,
    setup: &HttpSetup,
    timeout: Duration,
) -> Result<(u16, Duration), String> {
    let deadline = Instant::now() + timeout;
    let remaining = || {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            Err("timed out".to_string())
        } else {
            Ok(left)
        }
    };
    let host = match setup.host.parse::<IpAddr>() {
        Ok(IpAddr::V6(v6)) => format!("[{v6}]"),
        _ => setup.host.clone(),
    };
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: paping/{}\r\nConnection: close\r\n\r\n",
        setup.path,
        env!("CARGO_PKG_VERSION")
    );
    stream.set_timeouts(remaining()?).map_err(io_text)?;
    stream
        .write_all(request.as_bytes())
        .and_then(|()| stream.flush())
        .map_err(io_text)?;
    let sent = Instant::now();

    let mut line = Vec::new();
    let mut buf = [0u8; 512];
    let mut ttfb = None;
    loop {
        stream.set_timeouts(remaining()?).map_err(io_text)?;
        let n = stream.read(&mut buf).map_err(io_text)?;
        if n == 0 {
            return Err(if line.is_empty() {
                "connection closed".into()
            } else {
                "incomplete status line".into()
            });
        }
        ttfb.get_or_insert_with(|| sent.elapsed());
        line.extend_from_slice(&buf[..n]);
        if let Some(end) = line.windows(2).position(|w| w == b"\r\n") {
            line.truncate(end);
            break;
        }
        if line.len() > MAX_STATUS_LINE {
            return Err("status line too long".into());
        }
    }
    let status = parse_status(&line).ok_or_else(|| "not an HTTP response".to_string())?;
    Ok((status, ttfb.unwrap_or_default()))
}

const MAX_STATUS_LINE: usize = 8 * 1024;

/// `HTTP/1.x NNN reason` → `NNN`.
fn parse_status(line: &[u8]) -> Option<u16> {
    let line = std::str::from_utf8(line).ok()?;
    let mut parts = line.split(' ');
    if !parts.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let code = parts.next()?;
    if code.len() != 3 {
        return None;
    }
    code.parse().ok()
}

fn io_text(e: io::Error) -> String {
    match e.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => "timed out".into(),
        _ => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use crate::probe::{ProbeError, ProbeSpec, probe};
    use crate::tls;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::HttpSetup;

    /// Reads one request (up to the empty line), answers with `response`, and closes the
    /// connection only when `close` is set.
    fn http_server(response: &'static [u8], close: bool) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let mut open = Vec::new();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                read_request(&mut stream);
                let _ = stream.write_all(response);
                if !close {
                    open.push(stream);
                }
            }
        });
        addr
    }

    fn read_request(stream: &mut impl Read) {
        let mut seen = Vec::new();
        let mut byte = [0u8; 1];
        while !seen.ends_with(b"\r\n\r\n") {
            match stream.read(&mut byte) {
                Ok(1) => seen.push(byte[0]),
                _ => return,
            }
        }
    }

    fn spec(addr: SocketAddr, timeout_ms: u64) -> ProbeSpec {
        ProbeSpec {
            addr,
            bind: None,
            timeout: Duration::from_millis(timeout_ms),
            tls: None,
            http: Some(HttpSetup {
                host: "localhost".into(),
                path: "/".into(),
            }),
        }
    }

    #[test]
    fn http_200() {
        let r = probe(
            &spec(http_server(b"HTTP/1.1 200 OK\r\n\r\n", true), 2000),
            1,
        );
        assert!(r.ok(), "{:?}", r.error);
        assert_eq!(r.http_status, Some(200));
        assert!(r.ttfb.is_some());
        assert!(r.connect.is_some());
    }

    #[test]
    fn http_404_is_ok() {
        let r = probe(
            &spec(http_server(b"HTTP/1.1 404 Not Found\r\n\r\n", true), 2000),
            1,
        );
        assert!(r.ok(), "{:?}", r.error);
        assert_eq!(r.http_status, Some(404));
    }

    #[test]
    fn http_503_fails() {
        let r = probe(
            &spec(
                http_server(b"HTTP/1.1 503 Service Unavailable\r\n\r\n", true),
                2000,
            ),
            1,
        );
        assert_eq!(r.error, Some(ProbeError::HttpStatus(503)));
        assert_eq!(r.http_status, Some(503));
        assert!(r.ttfb.is_some());
    }

    #[test]
    fn http_garbage() {
        let r = probe(&spec(http_server(b"hello\r\n", true), 2000), 1);
        assert!(
            matches!(r.error, Some(ProbeError::Http(_))),
            "{:?}",
            r.error
        );
        assert_eq!(r.http_status, None);
    }

    #[test]
    fn http_silent_times_out() {
        let started = Instant::now();
        let r = probe(&spec(http_server(b"", false), 300), 1);
        assert!(
            matches!(r.error, Some(ProbeError::Http(_))),
            "{:?}",
            r.error
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn http_does_not_wait_for_body() {
        let started = Instant::now();
        let r = probe(&spec(http_server(b"HTTP/1.1 200 OK\r\n", false), 3000), 1);
        assert!(r.ok(), "{:?}", r.error);
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn http_timeout_bounds_the_whole_step() {
        // The status line arrives one byte every 300 ms: every single read is quick, but
        // the step as a whole must still end at -t.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                thread::spawn(move || {
                    read_request(&mut stream);
                    for byte in b"HTTP/1.1 200 OK\r\n" {
                        if stream.write_all(&[*byte]).is_err() {
                            return;
                        }
                        thread::sleep(Duration::from_millis(300));
                    }
                });
            }
        });
        let started = Instant::now();
        let r = probe(&spec(addr, 1000), 1);
        assert_eq!(r.error, Some(ProbeError::Http("timed out".into())));
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn request_has_host_path_and_agent() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut seen = Vec::new();
            let mut byte = [0u8; 1];
            while !seen.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                seen.push(byte[0]);
            }
            stream
                .write_all(b"HTTP/1.0 204 No Content\r\n\r\n")
                .unwrap();
            String::from_utf8(seen).unwrap()
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        let setup = HttpSetup {
            host: "example.com".into(),
            path: "/health".into(),
        };
        let (status, _) = super::request(&mut stream, &setup, Duration::from_secs(2)).unwrap();
        assert_eq!(status, 204);
        let sent = server.join().unwrap();
        assert!(sent.starts_with("GET /health HTTP/1.1\r\n"), "{sent}");
        assert!(sent.contains("\r\nHost: example.com\r\n"), "{sent}");
        assert!(
            sent.contains(&format!(
                "\r\nUser-Agent: paping/{}\r\n",
                env!("CARGO_PKG_VERSION")
            )),
            "{sent}"
        );
        assert!(sent.contains("\r\nConnection: close\r\n"), "{sent}");
    }

    #[test]
    fn https_200() {
        let mut s = spec(
            tls::test_support::tls_server(Some(b"HTTP/1.1 200 OK\r\n\r\n")),
            2000,
        );
        s.tls = Some(tls::setup("localhost", false));
        let r = probe(&s, 1);
        assert!(r.ok(), "{:?}", r.error);
        assert!(r.tls.is_some());
        assert_eq!(r.http_status, Some(200));
    }
}
