mod auth;
mod cli;
mod commands;
mod config;
mod error;
mod google;
mod output;

use clap::Parser;
use cli::Cli;
use error::AppError;
use output::SuccessEnvelope;
use serde_json::json;

#[tokio::main]
async fn main() {
    let result = run().await;
    let exit_code = match result {
        Ok(envelope) => match output::write_success(&envelope) {
            Ok(()) => 0,
            Err(error) => {
                let fallback = AppError::local_io(
                    "output",
                    "failed to write JSON output",
                    json!({ "ioError": error.to_string() }),
                );
                let _ = output::write_error(&fallback);
                fallback.exit_code
            }
        },
        Err(error) => {
            let exit_code = error.exit_code;
            let _ = output::write_error(&error);
            exit_code
        }
    };

    std::process::exit(exit_code);
}

async fn run() -> Result<SuccessEnvelope, AppError> {
    let args: Vec<String> = std::env::args().collect();

    if cli::contains_flag(&args, "--version") {
        return Ok(output::success(
            "version",
            None,
            json!({
                "name": env!("CARGO_PKG_NAME"),
                "version": env!("CARGO_PKG_VERSION"),
                "description": env!("CARGO_PKG_DESCRIPTION"),
            }),
            json!({}),
        ));
    }

    if cli::contains_flag(&args, "--help") {
        return Ok(output::success("help", None, cli::help_json(), json!({})));
    }

    let cli = Cli::try_parse_from(args).map_err(|error| {
        AppError::usage(
            "cli",
            "invalid command line",
            json!({ "clap": error.to_string() }),
        )
    })?;

    commands::run(cli).await
}
