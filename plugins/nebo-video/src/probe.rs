//! `probe`: ffprobe wrapper that returns duration, dimensions, fps, codecs.

use std::process::Command;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::ffmpeg;

#[derive(Deserialize)]
struct Input {
    input: String,
}

#[derive(Deserialize)]
struct FFProbeOut {
    format: Option<Format>,
    streams: Option<Vec<Stream>>,
}

#[derive(Deserialize)]
struct Format {
    duration: Option<String>,
    size: Option<String>,
    bit_rate: Option<String>,
}

#[derive(Deserialize)]
struct Stream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    r_frame_rate: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u32>,
}

pub fn run(input: &Value) -> Result<Value, String> {
    let args: Input = serde_json::from_value(input.clone())
        .map_err(|e| format!("probe: bad input: {e}"))?;

    let bin = ffmpeg::ffprobe_bin()?;
    let mut cmd = Command::new(bin);
    cmd.args([
        "-v", "error",
        "-print_format", "json",
        "-show_format",
        "-show_streams",
        &args.input,
    ]);
    let out = ffmpeg::run("ffprobe", cmd)?;

    let probed: FFProbeOut = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("probe: could not parse ffprobe output: {e}"))?;

    let duration = probed
        .format
        .as_ref()
        .and_then(|f| f.duration.as_deref())
        .and_then(|s| s.parse::<f64>().ok());
    let size = probed
        .format
        .as_ref()
        .and_then(|f| f.size.as_deref())
        .and_then(|s| s.parse::<u64>().ok());
    let bit_rate = probed
        .format
        .as_ref()
        .and_then(|f| f.bit_rate.as_deref())
        .and_then(|s| s.parse::<u64>().ok());

    let streams = probed.streams.unwrap_or_default();
    let video = streams.iter().find(|s| s.codec_type.as_deref() == Some("video"));
    let audio = streams.iter().find(|s| s.codec_type.as_deref() == Some("audio"));

    let fps = video.and_then(|v| v.r_frame_rate.as_deref()).map(parse_rational);

    Ok(json!({
        "input": args.input,
        "duration": duration,
        "size_bytes": size,
        "bit_rate": bit_rate,
        "width": video.and_then(|v| v.width),
        "height": video.and_then(|v| v.height),
        "fps": fps,
        "video_codec": video.and_then(|v| v.codec_name.clone()),
        "audio_codec": audio.and_then(|a| a.codec_name.clone()),
        "audio_sample_rate": audio.and_then(|a| a.sample_rate.as_deref())
            .and_then(|s| s.parse::<u32>().ok()),
        "audio_channels": audio.and_then(|a| a.channels),
    }))
}

/// ffprobe reports frame rates like "30000/1001" — convert to f64.
fn parse_rational(s: &str) -> f64 {
    let mut parts = s.split('/');
    let n = parts.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    let d = parts.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(1.0);
    if d == 0.0 { 0.0 } else { n / d }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rational_handles_common_forms() {
        assert!((parse_rational("30/1") - 30.0).abs() < 0.001);
        assert!((parse_rational("30000/1001") - 29.97).abs() < 0.01);
        assert_eq!(parse_rational("0/0"), 0.0);
        assert_eq!(parse_rational(""), 0.0);
    }
}
