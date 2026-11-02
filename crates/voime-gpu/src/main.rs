mod aed;
mod cli;
mod protocol;
mod runtime;
mod server;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args = cli::parse();
    match server::Server::run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("voime-gpu: {error:#}");
            ExitCode::from(1)
        }
    }
}
