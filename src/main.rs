use std::process::ExitCode;

fn main() -> ExitCode {
    let args = voime::cli::parse();
    match voime::run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("voime: {error:#}");
            ExitCode::from(1)
        }
    }
}
