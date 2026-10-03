//! `render`: compile a Project into one ffmpeg -filter_complex invocation.
//!
//! v0.1.0 scope: multi-clip video concat with uniform output scaling,
//! multi-track audio mixing, time-gated text overlays. No transitions, no
//! custom fonts, no gap filler. Those arrive in v0.2.0.

use std::collections::BTreeMap;
use std::fs;
use std::process::Command;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::ffmpeg;
use crate::project::{AudioClip, Project, TextClip, VideoClip};

#[derive(Deserialize)]
struct Input {
    project: String,
    output: String,
}

pub fn run(input: &Value) -> Result<Value, String> {
    let args: Input = serde_json::from_value(input.clone())
        .map_err(|e| format!("render: bad input: {e}"))?;

    let raw = fs::read_to_string(&args.project)
        .map_err(|e| format!("render: could not read project {}: {e}", args.project))?;
    let project: Project = serde_json::from_str(&raw)
        .map_err(|e| format!("render: invalid project JSON: {e}"))?;

    let plan = plan_render(&project, &args.output)?;
    let bin = ffmpeg::ffmpeg_bin()?;
    let mut cmd = Command::new(bin);
    cmd.args(["-y"]);
    for input in &plan.inputs {
        cmd.args(["-i", input]);
    }
    cmd.args(["-filter_complex", &plan.filter_complex]);
    for m in &plan.maps {
        cmd.args(["-map", m]);
    }
    cmd.args(&plan.encode_args);
    cmd.arg(&args.output);

    ffmpeg::run("ffmpeg", cmd)?;

    let size = fs::metadata(&args.output).map(|m| m.len()).unwrap_or(0);
    Ok(json!({
        "output": args.output,
        "size_bytes": size,
        "input_count": plan.inputs.len(),
    }))
}

#[derive(Debug)]
struct RenderPlan {
    inputs: Vec<String>,
    filter_complex: String,
    maps: Vec<String>,
    encode_args: Vec<String>,
}

fn plan_render(project: &Project, _output: &str) -> Result<RenderPlan, String> {
    // Deduplicate inputs so a single source file referenced many times only
    // opens once. The returned index is used in filter_complex labels.
    let mut input_index: BTreeMap<String, usize> = BTreeMap::new();
    let mut inputs: Vec<String> = Vec::new();
    let mut idx_for = |src: &str| -> usize {
        if let Some(i) = input_index.get(src) {
            *i
        } else {
            let i = inputs.len();
            input_index.insert(src.to_string(), i);
            inputs.push(src.to_string());
            i
        }
    };

    let out = &project.output;
    let mut graph = String::new();

    // ---- Video chain ----
    let video_clips: Vec<&VideoClip> =
        project.video_tracks().flat_map(|cs| cs.iter()).collect();
    if video_clips.is_empty() {
        return Err("render: project has no video clips".into());
    }

    let mut video_labels: Vec<String> = Vec::new();
    for (n, clip) in video_clips.iter().enumerate() {
        let i = idx_for(&clip.source);
        let label = format!("v{n}");
        // trim → setpts → scale to output → pad to output (letterbox)
        graph.push_str(&format!(
            "[{i}:v]trim=start={ts}:duration={d},setpts=PTS-STARTPTS,\
             scale={w}:{h}:force_original_aspect_ratio=decrease,\
             pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:black,\
             fps={fps}[{label}];",
            i = i, ts = clip.trim_start, d = clip.duration,
            w = out.width, h = out.height, fps = out.fps, label = label,
        ));
        video_labels.push(label);
    }
    for label in &video_labels {
        graph.push_str(&format!("[{label}]"));
    }
    graph.push_str(&format!("concat=n={}:v=1:a=0[vcat];", video_labels.len()));

    // ---- Text overlays, chained after [vcat] ----
    let text_clips: Vec<&TextClip> =
        project.text_tracks().flat_map(|cs| cs.iter()).collect();
    let final_video_label = if text_clips.is_empty() {
        "vcat".to_string()
    } else {
        let mut prev = "vcat".to_string();
        for (n, tc) in text_clips.iter().enumerate() {
            let next = format!("vt{n}");
            graph.push_str(&format!(
                "[{prev}]drawtext=text='{text}':fontsize={size}:fontcolor={color}:\
                 x=(w-text_w)*{x}:y=(h-text_h)*{y}:\
                 enable='between(t,{start},{end})'[{next}];",
                prev = prev, text = escape_drawtext(&tc.text),
                size = tc.size, color = tc.color,
                x = tc.x, y = tc.y,
                start = tc.start, end = tc.start + tc.duration,
                next = next,
            ));
            prev = next;
        }
        prev
    };

    // ---- Audio chain ----
    // Collect every audio clip across every audio track, trim+shift+gain each,
    // then amix. If no audio clips exist, we skip the audio chain and output
    // video-only.
    let audio_clips: Vec<&AudioClip> =
        project.audio_tracks().flat_map(|cs| cs.iter()).collect();
    let final_audio_label = if audio_clips.is_empty() {
        None
    } else {
        let mut labels: Vec<String> = Vec::new();
        for (n, clip) in audio_clips.iter().enumerate() {
            let i = idx_for(&clip.source);
            let label = format!("a{n}");
            let dur_arg = clip.duration.map(|d| format!(":duration={d}")).unwrap_or_default();
            let user_filter = clip
                .filter
                .as_deref()
                .map(|f| format!(",{f}"))
                .unwrap_or_default();
            // adelay expects ms per channel; use stereo by default.
            let delay_ms = (clip.start * 1000.0) as u64;
            graph.push_str(&format!(
                "[{i}:a]atrim=start={ts}{dur},asetpts=PTS-STARTPTS,\
                 volume={vol}{uf},adelay={d}|{d}[{label}];",
                i = i, ts = clip.trim_start, dur = dur_arg,
                vol = clip.volume, uf = user_filter,
                d = delay_ms, label = label,
            ));
            labels.push(label);
        }
        for label in &labels {
            graph.push_str(&format!("[{label}]"));
        }
        graph.push_str(&format!(
            "amix=inputs={}:dropout_transition=0:normalize=0[acat];",
            labels.len()
        ));
        Some("acat".to_string())
    };

    // ---- Final map + encode args ----
    let maps = if let Some(a) = &final_audio_label {
        vec![format!("[{final_video_label}]"), format!("[{a}]")]
    } else {
        vec![format!("[{final_video_label}]")]
    };

    let (vcodec, acodec) = match out.codec.as_str() {
        "h265" | "hevc" => ("libx265", "aac"),
        "vp9" => ("libvpx-vp9", "libopus"),
        "av1" => ("libaom-av1", "libopus"),
        _ => ("libx264", "aac"),
    };

    let mut encode_args = vec![
        "-c:v".into(), vcodec.into(),
        "-b:v".into(), out.video_bitrate.clone(),
        "-pix_fmt".into(), out.pixel_format.clone(),
        "-r".into(), format!("{}", out.fps),
    ];
    if final_audio_label.is_some() {
        encode_args.extend([
            "-c:a".into(), acodec.into(),
            "-b:a".into(), out.audio_bitrate.clone(),
        ]);
    }
    // Trailing semicolons in filter_complex are tolerated but let's be clean.
    while graph.ends_with(';') {
        graph.pop();
    }

    Ok(RenderPlan {
        inputs,
        filter_complex: graph,
        maps,
        encode_args,
    })
}

/// drawtext uses `:` to separate args and `'` to quote values. Escape both.
/// Also escape `\` and `%` (format specifiers).
fn escape_drawtext(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            ':' => out.push_str("\\:"),
            '%' => out.push_str("\\%"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{OutputSpec, Track};

    fn basic_output() -> OutputSpec {
        OutputSpec {
            width: 1920, height: 1080, fps: 30,
            codec: "h264".into(), video_bitrate: "5M".into(),
            audio_bitrate: "128k".into(), pixel_format: "yuv420p".into(),
        }
    }

    #[test]
    fn plans_a_single_video_clip() {
        let p = Project {
            output: basic_output(),
            tracks: vec![Track::Video {
                clips: vec![VideoClip {
                    source: "/a.mp4".into(),
                    start: 0.0, duration: 4.0, trim_start: 0.0,
                }],
            }],
        };
        let plan = plan_render(&p, "/out.mp4").unwrap();
        assert_eq!(plan.inputs, vec!["/a.mp4".to_string()]);
        assert!(plan.filter_complex.contains("concat=n=1:v=1:a=0[vcat]"));
        assert_eq!(plan.maps, vec!["[vcat]".to_string()]);
        assert!(plan.encode_args.iter().any(|a| a == "libx264"));
        // No audio.
        assert!(!plan.encode_args.iter().any(|a| a == "-c:a"));
    }

    #[test]
    fn plans_concatenated_video_clips_with_dedup_inputs() {
        let p = Project {
            output: basic_output(),
            tracks: vec![Track::Video {
                clips: vec![
                    VideoClip { source: "/a.mp4".into(), start: 0.0, duration: 2.0, trim_start: 0.0 },
                    VideoClip { source: "/a.mp4".into(), start: 2.0, duration: 3.0, trim_start: 5.0 },
                ],
            }],
        };
        let plan = plan_render(&p, "/out.mp4").unwrap();
        // Same source → one -i input.
        assert_eq!(plan.inputs.len(), 1);
        assert!(plan.filter_complex.contains("concat=n=2:v=1:a=0[vcat]"));
    }

    #[test]
    fn plans_video_plus_audio_adds_amix() {
        let p = Project {
            output: basic_output(),
            tracks: vec![
                Track::Video {
                    clips: vec![VideoClip {
                        source: "/a.mp4".into(), start: 0.0, duration: 4.0, trim_start: 0.0,
                    }],
                },
                Track::Audio {
                    clips: vec![AudioClip {
                        source: "/b.wav".into(), start: 1.0, trim_start: 0.0,
                        duration: Some(2.0), volume: 1.0, filter: None,
                    }],
                },
            ],
        };
        let plan = plan_render(&p, "/out.mp4").unwrap();
        assert_eq!(plan.inputs.len(), 2);
        assert!(plan.filter_complex.contains("amix=inputs=1"));
        assert_eq!(plan.maps, vec!["[vcat]".to_string(), "[acat]".to_string()]);
        assert!(plan.encode_args.iter().any(|a| a == "-c:a"));
    }

    #[test]
    fn text_overlay_chains_after_vcat() {
        let p = Project {
            output: basic_output(),
            tracks: vec![
                Track::Video {
                    clips: vec![VideoClip {
                        source: "/a.mp4".into(), start: 0.0, duration: 4.0, trim_start: 0.0,
                    }],
                },
                Track::Text {
                    clips: vec![TextClip {
                        text: "Hello".into(), start: 0.5, duration: 2.0,
                        size: 64, x: 0.5, y: 0.5, color: "white".into(),
                    }],
                },
            ],
        };
        let plan = plan_render(&p, "/out.mp4").unwrap();
        assert!(plan.filter_complex.contains("drawtext"));
        assert!(plan.filter_complex.contains("between(t,0.5,2.5)"));
        assert_eq!(plan.maps, vec!["[vt0]".to_string()]);
    }

    #[test]
    fn errors_when_no_video() {
        let p = Project {
            output: basic_output(),
            tracks: vec![Track::Audio { clips: vec![] }],
        };
        let err = plan_render(&p, "/out.mp4").unwrap_err();
        assert!(err.contains("no video clips"));
    }

    #[test]
    fn escapes_drawtext_special_chars() {
        assert_eq!(escape_drawtext("hello"), "hello");
        assert_eq!(escape_drawtext("a:b"), "a\\:b");
        assert_eq!(escape_drawtext("it's"), "it\\'s");
        assert_eq!(escape_drawtext("100%"), "100\\%");
    }
}
