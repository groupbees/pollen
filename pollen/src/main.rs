use std::process::ExitCode;

fn main() -> ExitCode {
    ExitCode::from(pollen::cli::main(std::env::args_os()))
}
