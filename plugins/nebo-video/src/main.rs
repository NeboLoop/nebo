//! nebo-video: ffmpeg-backed video assembly plugin.
//!
//! Protocol: reads a JSON object on stdin, writes a JSON object on stdout.
//! The subcommand (one of `probe`, `trim`, `render`) is the only argv.
//! Exits 0 on success, 1 on error (the error object is still written to stdout).

use std::io::{self, Read, Write};
use std::process::ExitCode;

use serde_json::{Value, json};

mod ffmpeg;
mod probe;
mod project;
mod render;
mod trim;

fn main() -> ExitCode {
    let mut raw = String::new();
    if io::stdin().read_to_string(&mut raw).is_err() {
        return emit_err("could not read stdin");
    }
    let input: Value = if raw.trim().is_empty() {
        Value::Object(Default::default())
    } else {
        match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => return emit_err(&format!("invalid JSON on stdin: {e}")),
        }
    };

    let action = std::env::args().nth(1).unwrap_or_default();
    let result = match action.as_str() {
        "probe" => probe::run(&input),
        "trim" => trim::run(&input),
        "render" => render::run(&input),
        "" => Err("missing subcommand (probe | trim | render)".to_string()),
        other => Err(format!("unknown subcommand: {other}")),
    };

    match result {
        Ok(v) => {
            let _ = writeln!(io::stdout(), "{}", v);
            ExitCode::SUCCESS
        }
        Err(e) => emit_err(&e),
    }
}

fn emit_err(msg: &str) -> ExitCode {
    let _ = writeln!(io::stdout(), "{}", json!({ "error": msg }));
    ExitCode::from(1)
}
