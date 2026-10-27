use std::io;
use std::net::{IpAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};

/// Everything a probe needs to run a TLS handshake toward one target.
#[derive(Debug, Clone)]
pub struct TlsSetup {
    config: Arc<ClientConfig>,
    server_name: ServerName<'static>,
}

/// Names are sent as SNI; IP literals are not (RFC 6066 forbids IP addresses in SNI).
pub fn setup(host: &str, is_literal: bool) -> TlsSetup {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
        .with_no_client_auth();
    let server_name = match (is_literal, host.parse::<IpAddr>()) {
        (true, Ok(ip)) => ServerName::IpAddress(ip.into()),
        // A name rustls rejects as SNI (e.g. with an underscore) still gets measured,
        // just without SNI.
        _ => ServerName::try_from(host.to_string())
            .unwrap_or_else(|_| ServerName::IpAddress(IpAddr::from([0, 0, 0, 0]).into())),
    };
    TlsSetup {
        config: Arc::new(config),
        server_name,
    }
}

/// A TCP stream after a finished handshake, ready for `--http`.
pub type TlsStream = StreamOwned<ClientConnection, TcpStream>;

/// Runs a TLS handshake on a connected stream and returns how long it took, plus the
/// stream for anything that follows. The certificate is not verified: paping measures
/// time, it does not judge trust.
pub fn handshake(
    mut stream: TcpStream,
    setup: &TlsSetup,
    timeout: Duration,
) -> Result<(Duration, TlsStream), String> {
    let started = Instant::now();
    let deadline = started + timeout;
    let mut conn = ClientConnection::new(setup.config.clone(), setup.server_name.clone())
        .map_err(|e| e.to_string())?;
    while conn.is_handshaking() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("timed out".into());
        }
        stream
            .set_read_timeout(Some(remaining))
            .and_then(|()| stream.set_write_timeout(Some(remaining)))
            .map_err(|e| e.to_string())?;
        if let Err(e) = conn.complete_io(&mut stream) {
            return Err(match e.kind() {
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => "timed out".into(),
                _ => e.to_string(),
            });
        }
    }
    Ok((started.elapsed(), StreamOwned::new(conn, stream)))
}

#[derive(Debug)]
struct NoVerify(Arc<CryptoProvider>);

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
impl TlsSetup {
    fn server_name(&self) -> &ServerName<'static> {
        &self.server_name
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener};
    use std::sync::Arc;
    use std::thread;

    /// A TLS server on 127.0.0.1 with a fresh self-signed certificate. It completes the
    /// handshake on every accepted connection. With `response`, it then reads one HTTP
    /// request and writes `response`.
    pub(crate) fn tls_server(response: Option<&'static [u8]>) -> SocketAddr {
        use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
        use rustls::{ServerConfig, ServerConnection, StreamOwned};

        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()));
        let config = Arc::new(
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![cert.cert.der().clone()], key)
                .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let config = config.clone();
                thread::spawn(move || {
                    let mut conn = ServerConnection::new(config).unwrap();
                    while conn.is_handshaking() {
                        if conn.complete_io(&mut stream).is_err() {
                            return;
                        }
                    }
                    let Some(response) = response else {
                        let _ = conn.complete_io(&mut stream);
                        return;
                    };
                    let mut tls = StreamOwned::new(conn, stream);
                    let mut seen = Vec::new();
                    let mut byte = [0u8; 1];
                    while !seen.ends_with(b"\r\n\r\n") {
                        match tls.read(&mut byte) {
                            Ok(1) => seen.push(byte[0]),
                            _ => return,
                        }
                    }
                    let _ = tls.write_all(response).and_then(|()| tls.flush());
                });
            }
        });
        addr
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{ProbeError, ProbeSpec, probe};
    use std::net::{SocketAddr, TcpListener};
    use std::thread;
    use test_support::tls_server;

    fn spec(addr: SocketAddr, timeout_ms: u64) -> ProbeSpec {
        ProbeSpec {
            addr,
            bind: None,
            timeout: Duration::from_millis(timeout_ms),
            tls: Some(setup("localhost", false)),
            http: None,
        }
    }

    #[test]
    fn tls_handshake_measured() {
        let r = probe(&spec(tls_server(None), 2000), 1);
        assert!(r.ok(), "{:?}", r.error);
        assert!(r.connect.is_some());
        assert!(r.tls.is_some());
    }

    #[test]
    fn tls_against_plain_tcp_fails() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming() {
                drop(stream);
            }
        });
        let r = probe(&spec(addr, 2000), 1);
        assert!(matches!(r.error, Some(ProbeError::Tls(_))), "{:?}", r.error);
        assert!(r.connect.is_some());
        assert!(!r.ok());
    }

    #[test]
    fn tls_against_silent_server_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let mut open = Vec::new();
            for stream in listener.incoming() {
                open.push(stream); // keep it open, never answer
            }
        });
        let started = Instant::now();
        let r = probe(&spec(addr, 300), 1);
        assert!(matches!(r.error, Some(ProbeError::Tls(_))), "{:?}", r.error);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn setup_literal_has_no_sni() {
        assert!(matches!(
            setup("127.0.0.1", true).server_name(),
            ServerName::IpAddress(_)
        ));
        assert!(matches!(
            setup("example.com", false).server_name(),
            ServerName::DnsName(_)
        ));
    }
}
