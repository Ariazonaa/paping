mod common;

use std::io::{BufRead, BufReader, Read};
use std::process::{Command as StdCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::cargo::{cargo_bin, cargo_bin_cmd};
use serde_json::Value;

fn paping() -> assert_cmd::Command {
    cargo_bin_cmd!("paping")
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn connects_and_exits_zero() {
    let addr = common::listener();
    let port = addr.port().to_string();
    let out = paping()
        .args([
            "127.0.0.1",
            "-p",
            &port,
            "-c",
            "2",
            "--interval",
            "50",
            "--nocolor",
        ])
        .output()
        .unwrap();
    let stdout = stdout_of(&out);
    let version = env!("CARGO_PKG_VERSION");
    assert!(
        stdout.starts_with(&format!(
            "paping v{version}\n\nConnecting to 127.0.0.1 on TCP {port}:\n\n"
        )),
        "{stdout}"
    );
    assert_eq!(stdout.matches("Connected to 127.0.0.1: time=").count(), 2);
    assert!(stdout.contains("\tAttempted = 2, Connected = 2, Failed = 0 (0.00%)\n"));
    assert!(!stdout.contains('\x1b'), "no colors when piped");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn closed_port_exits_one() {
    let port = common::closed_port().port().to_string();
    let out = paping()
        .args(["127.0.0.1", "-p", &port, "-c", "1", "-t", "3000"])
        .output()
        .unwrap();
    let stdout = stdout_of(&out);
    assert!(stdout.contains("\nConnection refused\n"), "{stdout}");
    assert!(stdout.contains("Failed = 1 (100.00%)"));
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn usage_error_exits_two() {
    let out = paping().output().unwrap();
    assert!(stdout_of(&out).contains("Syntax: paping [options] destination"));
    assert_eq!(stderr_of(&out).trim(), "Error: destination is required");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn help_exits_zero() {
    let out = paping().arg("-?").output().unwrap();
    let stdout = stdout_of(&out);
    assert!(stdout.starts_with(&format!(
        "paping v{}\n\nSyntax: paping",
        env!("CARGO_PKG_VERSION")
    )));
    assert!(stdout.contains("Extended options:"));
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn version_exits_zero() {
    let out = paping().arg("--version").output().unwrap();
    assert_eq!(
        stdout_of(&out),
        format!("paping v{}\n", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn tui_needs_a_terminal() {
    let out = paping()
        .args(["127.0.0.1", "-p", "80", "--tui"])
        .output()
        .unwrap();
    assert_eq!(stderr_of(&out).trim(), "Error: --tui needs a terminal");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn unresolvable_exits_two() {
    let out = paping()
        .args(["does-not-exist.invalid", "-p", "80"])
        .output()
        .unwrap();
    assert!(stdout_of(&out).contains("Cannot resolve host"));
    assert_eq!(out.status.code(), Some(2));

    let out = paping()
        .args(["does-not-exist.invalid", "-p", "80", "--json"])
        .output()
        .unwrap();
    let stdout = stdout_of(&out);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "{stdout}");
    let v: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(v["type"], "error");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn json_mode() {
    let port = common::listener().port().to_string();
    let out = paping()
        .args([
            "127.0.0.1",
            "-p",
            &port,
            "-c",
            "2",
            "--interval",
            "50",
            "--json",
        ])
        .output()
        .unwrap();
    let stdout = stdout_of(&out);
    let types: Vec<String> = stdout
        .lines()
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            v["type"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(types, ["start", "probe", "probe", "summary"]);
    assert!(!stdout.contains("paping v"));
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn csv_mode() {
    let port = common::listener().port().to_string();
    let out = paping()
        .args([
            "127.0.0.1",
            "-p",
            &port,
            "-c",
            "2",
            "--interval",
            "50",
            "--csv",
        ])
        .output()
        .unwrap();
    let stdout = stdout_of(&out);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{stdout}");
    assert_eq!(
        lines[0],
        "seq,time,ip,port,ok,connect_ms,tls_ms,error,http_status,ttfb_ms"
    );
    assert!(lines[1].starts_with("1,"));
    assert!(lines[2].starts_with("2,"));
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn tls_mode() {
    let port = common::tls_server().port().to_string();
    let out = paping()
        .args(["127.0.0.1", "-p", &port, "-c", "1", "--tls", "--nocolor"])
        .output()
        .unwrap();
    let stdout = stdout_of(&out);
    assert!(stdout.contains(" tls="), "{stdout}");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn details_mode() {
    let port = common::listener().port().to_string();
    let out = paping()
        .args(["localhost", "-p", &port, "-4", "-c", "1", "--details"])
        .output()
        .unwrap();
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains("Connecting to localhost [127.0.0.1] on TCP"),
        "{stdout}"
    );
    assert!(stdout.contains("\nDNS lookup: "), "{stdout}");
    assert!(stdout.contains("\tMedian = "), "{stdout}");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn broken_pipe_is_quiet() {
    let port = common::listener().port().to_string();
    let mut child = StdCommand::new(cargo_bin!("paping"))
        .args(["127.0.0.1", "-p", &port, "--interval", "10"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    for _ in 0..4 {
        line.clear();
        reader.read_line(&mut line).unwrap();
    }
    drop(reader); // closes the pipe

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(2) {
            let _ = child.kill();
            panic!("paping kept running after its stdout was closed");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn ip_bind_v4_with_a_name_uses_ipv4() {
    // `localhost` may resolve to ::1 first; an IPv4 source address must still connect.
    let port = common::listener().port().to_string();
    let out = paping()
        .args([
            "localhost",
            "-p",
            &port,
            "-c",
            "1",
            "--ip-bind",
            "127.0.0.1",
        ])
        .output()
        .unwrap();
    let stdout = stdout_of(&out);
    assert!(stdout.contains("Connected to 127.0.0.1:"), "{stdout}");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn pipe_has_no_terminal_extras() {
    let port = common::listener().port().to_string();
    let out = paping()
        .args([
            "127.0.0.1",
            "-p",
            &port,
            "-c",
            "3",
            "--interval",
            "20",
            "--details",
        ])
        .output()
        .unwrap();
    let stdout = stdout_of(&out);
    assert!(!stdout.contains('\r'), "{stdout:?}");
    assert!(!stdout.contains("\x1b["), "{stdout:?}");
    assert!(!stdout.contains(" sent ·"), "{stdout:?}");
    assert!(!stdout.contains("Trend:"), "{stdout:?}");
    assert!(!stdout.contains('▁') && !stdout.contains('█'), "{stdout:?}");
}

#[test]
fn http_mode_end_to_end() {
    use std::io::Write as _;
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut stream, &mut buf);
            let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\n\r\n");
        }
    });
    let out = paping()
        .args(["127.0.0.1", "-p", &port, "-c", "1", "--http", "--details"])
        .output()
        .unwrap();
    let stdout = stdout_of(&out);
    assert!(stdout.contains(" http=204 ttfb="), "{stdout}");
    assert!(
        stdout.contains("Approximate HTTP response times:"),
        "{stdout}"
    );
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn dual_stack_lines_and_blocks() {
    if !common::localhost_is_dual_stack() {
        return;
    }
    let Some(port) = common::dual_listener() else {
        return;
    };
    let port = port.to_string();
    let out = paping()
        .args(["localhost", "-p", &port, "-c", "2", "--interval", "50"])
        .output()
        .unwrap();
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains(&format!(
            "Connecting to localhost [127.0.0.1, ::1] on TCP {port}:"
        )),
        "{stdout}"
    );
    let lines: Vec<&str> = stdout
        .lines()
        .filter(|l| l.starts_with("Connected to"))
        .collect();
    assert_eq!(lines.len(), 4, "{stdout}");
    assert!(lines[0].starts_with("Connected to 127.0.0.1:"), "{stdout}");
    assert!(lines[1].starts_with("Connected to ::1:"), "{stdout}");
    assert!(lines[2].starts_with("Connected to 127.0.0.1:"), "{stdout}");
    assert!(lines[3].starts_with("Connected to ::1:"), "{stdout}");
    assert!(stdout.contains("\nConnection statistics (127.0.0.1):\n\tAttempted = 2,"));
    assert!(stdout.contains("\nConnection statistics (::1):\n\tAttempted = 2,"));
    assert_eq!(out.status.code(), Some(0));
}

/// One family answers HTTP at once, the other accepts and stays silent until the timeout.
/// Both must still start each round together instead of drifting apart.
#[test]
fn dual_stack_rounds_stay_in_step() {
    use std::io::Write as _;
    use std::net::TcpListener;
    if !common::localhost_is_dual_stack() {
        return;
    }
    let mut pair = None;
    for _ in 0..20 {
        let v4 = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = v4.local_addr().unwrap().port();
        if let Ok(v6) = TcpListener::bind(("::1", port)) {
            pair = Some((v4, v6, port));
            break;
        }
    }
    let Some((v4, v6, port)) = pair else {
        return;
    };
    thread::spawn(move || {
        for stream in v4.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut stream, &mut buf);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n");
        }
    });
    thread::spawn(move || {
        let mut open = Vec::new();
        for stream in v6.incoming() {
            open.push(stream);
        }
    });
    let port = port.to_string();
    let out = paping()
        .args([
            "localhost",
            "-p",
            &port,
            "--http",
            "-t",
            "500",
            "--interval",
            "100",
            "-c",
            "4",
            "--json",
        ])
        .output()
        .unwrap();
    let probes: Vec<Value> = stdout_of(&out)
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["type"] == "probe")
        .collect();
    assert_eq!(probes.len(), 8, "{probes:?}");
    let started = |v: &Value| -> jiff::Timestamp { v["time"].as_str().unwrap().parse().unwrap() };
    for round in probes.chunks(2) {
        assert_eq!(round[0]["ip"], "127.0.0.1");
        assert_eq!(round[1]["ip"], "::1");
        assert_eq!(round[0]["seq"], round[1]["seq"]);
        let gap = (started(&round[1]) - started(&round[0]))
            .total(jiff::Unit::Millisecond)
            .unwrap()
            .abs();
        assert!(gap < 100.0, "round {} drifted by {gap} ms", round[0]["seq"]);
    }
}

#[test]
fn dual_stack_one_family_refused() {
    if !common::localhost_is_dual_stack() {
        return;
    }
    let port = common::listener().port().to_string(); // IPv4 only
    let out = paping()
        .args(["localhost", "-p", &port, "-c", "1", "-t", "3000"])
        .output()
        .unwrap();
    let stdout = stdout_of(&out);
    let results: Vec<&str> = stdout
        .lines()
        .filter(|l| l.starts_with("Connected to") || l.starts_with("Connection refused"))
        .collect();
    assert_eq!(results.len(), 2, "{stdout}");
    assert!(
        results[0].starts_with("Connected to 127.0.0.1:"),
        "{stdout}"
    );
    assert_eq!(results[1], "Connection refused (::1)", "{stdout}");
    assert!(stdout.contains("Connection statistics (::1):\n\tAttempted = 1, Connected = 0"));
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn closed_stdout_before_resolve_error_is_quiet() {
    let mut child = StdCommand::new(cargo_bin!("paping"))
        .args(["does-not-exist.invalid", "-p", "80"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take()); // the reader goes away before anything is printed
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(out.status.code(), Some(2));
}

/// Ctrl+C while an attempt hangs: the TLS handshake toward a server that never answers
/// would take the full 10 s timeout, but the statistics must appear at once.
#[cfg(unix)]
#[test]
fn interrupt_prints_stats_immediately() {
    let port = common::silent_server().port().to_string();
    let child = StdCommand::new(cargo_bin!("paping"))
        .args(["127.0.0.1", "-p", &port, "--tls", "-t", "10000"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    StdCommand::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("Connection statistics:")
    );
}
