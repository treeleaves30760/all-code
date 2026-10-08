mod agents;
mod bridge;
mod bridge_host;
mod cli;
mod config;
mod doctor;
mod file_lock;
mod launch;
mod model_catalog;
mod model_picker;
mod ollama;
mod openai_server;
mod remote;
mod runtime;
mod tui;
mod update;
mod usage;

use std::process::ExitCode;

fn main() -> ExitCode {
    match runtime::early_dispatch() {
        Ok(Some(code)) => return code,
        Ok(None) => {}
        Err(error) => {
            eprintln!("error: {error:#}");
            return ExitCode::FAILURE;
        }
    }
    match cli::run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
