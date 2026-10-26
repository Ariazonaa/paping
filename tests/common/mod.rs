#![allow(dead_code)] // each test binary uses a different subset

use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::thread;

/// A listener on 127.0.0.1 that accepts and drops connections in the background.
pub fn listener() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    thread::spawn(move || for _ in l.incoming() {});
    addr
}

/// Listeners on 127.0.0.1 and [::1] with the same port, accepting in the background.
/// `None` when this machine has no IPv6 loopback.
pub fn dual_listener() -> Option<u16> {
    for _ in 0..20 {
        let v4 = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = v4.local_addr().unwrap().port();
        let Ok(v6) = TcpListener::bind(("::1", port)) else {
            if TcpListener::bind("[::1]:0").is_err() {
                return None; // no IPv6 at all
            }
            continue; // port taken on ::1, try another
        };
        thread::spawn(move || for _ in v4.incoming() {});
        thread::spawn(move || for _ in v6.incoming() {});
        return Some(port);
    }
    None
}

/// Whether `localhost` resolves to both 127.0.0.1 and ::1 here.
pub fn localhost_is_dual_stack() -> bool {
    use std::net::ToSocketAddrs;
    let addrs: Vec<_> = ("localhost", 0).to_socket_addrs().unwrap().collect();
    addrs.iter().any(|a| a.is_ipv4()) && addrs.iter().any(|a| a.is_ipv6())
}

/// A port on 127.0.0.1 that nothing listens on.
pub fn closed_port() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

/// A server on 127.0.0.1 that accepts connections, keeps them open and never sends a byte.
pub fn silent_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let mut open = Vec::new();
        for stream in listener.incoming() {
            open.push(stream);
        }
    });
    addr
}

/// A TLS server on 127.0.0.1 with a fresh self-signed certificate.
pub fn tls_server() -> SocketAddr {
    use rustls::ServerConfig;
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

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
                let mut conn = rustls::ServerConnection::new(config).unwrap();
                while conn.is_handshaking() {
                    if conn.complete_io(&mut stream).is_err() {
                        return;
                    }
                }
                let _ = conn.complete_io(&mut stream);
            });
        }
    });
    addr
}
