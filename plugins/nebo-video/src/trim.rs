//! `trim`: cut [start..end] from a file. Stream-copy by default, re-encode on request.

use std::fs;
use std::process::Command;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::ffmpeg;

#[derive(Deserialize)]
struct Input {
    input: String,
    start: f64,
    end: f64,
    output: String,
    #[serde(default)]
    reencode: bool,
}

pub fn run(input: &Value) -> Result<Value, String> {
    let args: Input = serde_json::from_value(input.clone())
        .map_err(|e| format!("trim: bad input: {e}"))?;

    if args.end <= args.start {
        return Err(format!(
            "trim: end ({}) must be greater than start ({})",
            args.end, args.start
        ));
    }
    let duration = args.end - args.start;

    let bin = ffmpeg::ffmpeg_bin()?;
    let mut cmd = Command::new(bin);
    // -ss BEFORE -i uses the demuxer's seek (fast, keyframe-aligned).
    // Precede -i with -ss and use -t for duration — simplest reliable form.
    cmd.args([
        "-y",
        "-ss", &format!("{}", args.start),
        "-i", &args.input,
        "-t", &format!("{}", duration),
    ]);
    if args.reencode {
        cmd.args(["-c:v", "libx264", "-c:a", "aac", "-preset", "medium"]);
    } else {
        cmd.args(["-c", "copy", "-avoid_negative_ts", "make_zero"]);
    }
    cmd.arg(&args.output);

    ffmpeg::run("ffmpeg", cmd)?;

    let size = fs::metadata(&args.output)
        .map(|m| m.len())
        .unwrap_or(0);

    Ok(json!({
        "output": args.output,
        "duration": duration,
        "size_bytes": size,
        "mode": if args.reencode { "reencode" } else { "stream_copy" },
    }))
}
