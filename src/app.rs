use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::mpsc;

use crate::cli::{self, Command, Config, OutputMode, USAGE, banner};
use crate::engine::{self, EngineHandle, Event};
use crate::http::HttpSetup;
use crate::output::csv::CsvSink;
use crate::output::json::{JsonSink, json_error_line};
use crate::output::text::{self, TextSink};
use crate::output::{Sink, use_color, use_interactive};
use crate::probe::ProbeSpec;
use crate::resolve::{self, DNS_TIMEOUT, Target};
use crate::stats::StatsSet;
use crate::{tls, tui};

const RESOLVE_ERROR: &str = "Cannot resolve host";

/// Runs paping with the arguments after `argv[0]` and returns the exit code:
/// 0 all attempts connected, 1 at least one failed, 2 usage error or unresolvable host.
pub fn run<I: IntoIterator<Item = OsString>>(args: I) -> ExitCode {
    let cfg = match cli::parse(args) {
        Ok(Command::Run(cfg)) => cfg,
        Ok(Command::Help) => {
            say(&format!("{}\n\n{USAGE}\n", banner()));
            return ExitCode::SUCCESS;
        }
        Ok(Command::Version) => {
            say(&format!("{}\n", banner()));
            return ExitCode::SUCCESS;
        }
        Err(cli::CliError(msg)) => return usage_error(&msg),
    };
    if cfg.output == OutputMode::Tui && !io::stdout().is_terminal() {
        return usage_error("--tui needs a terminal");
    }

    let color = matches!(cfg.output, OutputMode::Text | OutputMode::Tui) && use_color(cfg.color);
    if cfg.output == OutputMode::Text {
        say(&format!("{}\n\n", banner()));
    }

    let target = match resolve::resolve(&cfg.destination, cfg.family, DNS_TIMEOUT) {
        Ok(target) => target,
        Err(_) => {
            match cfg.output {
                OutputMode::Text => {
                    say(&format!(
                        "{}\n",
                        text::paint(RESOLVE_ERROR, text::RED, color)
                    ));
                }
                OutputMode::Json => say(&format!("{}\n", json_error_line(RESOLVE_ERROR))),
                OutputMode::Csv | OutputMode::Tui => eprintln!("{RESOLVE_ERROR}"),
            }
            return ExitCode::from(2);
        }
    };

    if cfg.output == OutputMode::Tui {
        let (events, engine) = start_engine(&cfg, &target);
        return match tui::run(&cfg, target, events, std::slice::from_ref(&engine)) {
            Ok(stats) => {
                say(&format!(
                    "{}\n",
                    text::stats_blocks(&stats, &cfg, color, true)
                ));
                exit_code(&stats)
            }
            Err(e) => {
                eprintln!("Error: {e}");
                ExitCode::from(2)
            }
        };
    }

    let mut sink: Box<dyn Sink> = match cfg.output {
        OutputMode::Json => Box::new(JsonSink::new(io::stdout())),
        OutputMode::Csv => Box::new(CsvSink::new(io::stdout())),
        OutputMode::Text | OutputMode::Tui => {
            let interactive = use_interactive();
            Box::new(TextSink::new(io::stdout(), color, interactive))
        }
    };
    let stats = run_sink(sink.as_mut(), &cfg, &target);
    exit_code(&stats)
}

/// Feeds the sink until the run ends. The engine delivers each round in address order.
/// A write error (e.g. a closed pipe) ends it early and quietly; the statistics so far
/// still decide the exit code.
fn run_sink(sink: &mut dyn Sink, cfg: &Config, target: &Target) -> StatsSet {
    let mut stats = StatsSet::new(&target.addrs);
    let (events, _engine) = start_engine(cfg, target);
    if sink.start(target, cfg).is_err() {
        return stats;
    }
    for event in events {
        match event {
            Event::Probe(r) => {
                stats.record(&r);
                if sink.probe(&r, target, cfg).is_err() {
                    return stats;
                }
            }
            Event::Done | Event::Interrupted => break,
        }
    }
    let _ = sink.finish(&stats, cfg);
    stats
}

/// Starts the probe thread (one round = every address at once) and the Ctrl+C handler;
/// both report through the returned channel.
fn start_engine(cfg: &Config, target: &Target) -> (mpsc::Receiver<Event>, EngineHandle) {
    let (tx, rx) = mpsc::channel();
    let interrupt = tx.clone();
    let _ = ctrlc::set_handler(move || {
        let _ = interrupt.send(Event::Interrupted);
    });
    let tls = cfg.tls.then(|| tls::setup(&target.host, target.is_literal));
    let http = cfg.http.then(|| HttpSetup {
        host: target.host.clone(),
        path: cfg.http_path.clone(),
    });
    let specs = target
        .addrs
        .iter()
        .map(|addr| ProbeSpec {
            addr: SocketAddr::new(*addr, cfg.port),
            bind: cfg.bind,
            timeout: cfg.timeout,
            tls: tls.clone(),
            http: http.clone(),
        })
        .collect();
    // Never joined: on Ctrl+C the statistics must not wait for an attempt in flight.
    let engine = engine::spawn(specs, cfg.count, cfg.interval, tx);
    (rx, engine)
}

/// Writes to stdout and ignores errors: a closed pipe (`| head`) must not panic. The
/// exit code still reflects the run.
fn say(text: &str) {
    let mut out = io::stdout();
    let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
}

fn exit_code(stats: &StatsSet) -> ExitCode {
    if stats.failed() == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// Like the original: banner and usage on stdout. The reason goes to stderr.
fn usage_error(msg: &str) -> ExitCode {
    say(&format!("{}\n\n{USAGE}\n", banner()));
    eprintln!("Error: {msg}");
    ExitCode::from(2)
}
