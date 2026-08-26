mod cli;
mod config;
mod doctor;
mod launch;
mod model_catalog;
mod model_picker;
mod tui;
mod update;

use std::process::ExitCode;

fn main() -> ExitCode {
    match cli::run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
