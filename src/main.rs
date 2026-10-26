use std::process::ExitCode;

fn main() -> ExitCode {
    paping::app::run(std::env::args_os().skip(1))
}
