//! `ling-mem` binary entry.
//!
//! Parses the CLI, dispatches to the subcommand handler, and renders errors
//! as JSON on stderr with a non-zero exit — matches `doc/tech-spec.md`'s
//! I/O contract so models and shell scripts can parse failures uniformly.

use clap::Parser;
use ling_mem::cli::{error_code, run, Cli};
use std::io::Write;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    // `tracing_subscriber` is optional; the CLI is mostly silent by default
    // and routes progress through stderr when `-v` is set. Skipping setup
    // keeps the binary quiet in piped contexts.
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let code = error_code(&err);
            let payload = serde_json::json!({
                "error": err.to_string(),
                "code": code,
            });
            let mut stderr = std::io::stderr().lock();
            let _ = serde_json::to_writer(&mut stderr, &payload);
            let _ = stderr.write_all(b"\n");
            ExitCode::from(1)
        }
    }
}
