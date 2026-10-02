//! A video the owner attaches, read the way a picture is: by the vision
//! helper (`sidecar`), never by the employee's own model.
//!
//! ffmpeg takes a few frames from it (about one a second, a few more where
//! the picture changes, at most [`MAX_FRAMES`], scaled down), and the helper
//! watches them in order, each with the moment it was taken, and writes what
//! happens. The frames stay on disk under the upload store, and the reading
//! names each one, so the employee can look at a frame again with its file
//! tools. Its sound, when it has any, is transcribed where every attachment's
//! sound is (the server's attachment notes): [`sound`] hands that path the
//! sound track alone.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ai::image_norm::Picture;

/// The most frames the helper watches.
pub const MAX_FRAMES: usize = 12;

/// The longest side of a frame, in pixels: enough to read a phone screen's
/// text, small enough that a dozen frames are one modest request.
const LONG_SIDE: u32 = 1024;

/// The most frames ffmpeg writes before they are thinned to [`MAX_FRAMES`].
const MAX_TAKEN: usize = 48;

/// The longest ffmpeg may take over one video.
const FFMPEG_LIMIT: Duration = Duration::from_secs(30);

/// Below this loudness (dBFS) a sound track is silence: a screen recording
/// with the microphone off still carries one, and a transcriber asked to
/// listen to silence invents words.
const SILENT_DB: f64 = -50.0;

/// Containers that only ever hold video.
const VIDEO_ONLY: &[&str] = &["mov", "m4v", "mkv", "avi", "wmv", "3gp", "mts", "m2ts"];

/// Containers that hold either; a declared `audio/` type says which.
const VIDEO_OR_AUDIO: &[&str] = &["mp4", "webm", "mpeg", "mpg", "ogv"];

/// Whether an attachment is a video.
pub fn is_video(filename: &str, mime_type: &str) -> bool {
    if mime_type.starts_with("video/") {
        return true;
    }
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    VIDEO_ONLY.contains(&ext.as_str())
        || (VIDEO_OR_AUDIO.contains(&ext.as_str()) && !mime_type.starts_with("audio/"))
}

/// One frame taken from a video: when, in seconds from its start, and the
/// file it was written to.
#[derive(Debug, Clone)]
pub struct Frame {
    pub at: f64,
    pub path: PathBuf,
}

/// A moment in a video as people say it: `0:03`, `1:02.5`.
pub fn clock(secs: f64) -> String {
    let tenths = (secs.max(0.0) * 10.0).round() as u64;
    let (whole, tenth) = (tenths / 10, tenths % 10);
    let stamp = format!("{}:{:02}", whole / 60, whole % 60);
    if tenth == 0 {
        stamp
    } else {
        format!("{stamp}.{tenth}")
    }
}

/// Where the frames taken from an attachment are kept: beside the upload
/// store, inside the folder the employee's file tools read.
pub fn frames_dir(file_id: &str) -> Option<PathBuf> {
    Some(crate::uploads::dir()?.join("frames").join(file_id))
}

/// ffmpeg on this machine, or the plain words for its absence.
fn ffmpeg() -> Result<PathBuf, String> {
    #[cfg(test)]
    if tests::NO_FFMPEG.with(|n| n.get()) {
        return Err(NO_FFMPEG.to_string());
    }
    which::which("ffmpeg").map_err(|_| NO_FFMPEG.to_string())
}

/// What a note says when this machine has no ffmpeg.
const NO_FFMPEG: &str = "ffmpeg is not installed on this computer";

/// Run ffmpeg with `args`, bounded by [`FFMPEG_LIMIT`]: its stdout and its
/// stderr (where it says what it saw), or why it could not run.
async fn run(args: &[&str]) -> Result<(bool, Vec<u8>, String), String> {
    let program = ffmpeg()?;
    let child = command::new::<tokio::process::Command>(&program, command::Console::Hidden)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(FFMPEG_LIMIT, child).await {
        Ok(Ok(o)) => Ok((
            o.status.success(),
            o.stdout,
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )),
        Ok(Err(e)) => Err(format!("ffmpeg could not run ({e})")),
        Err(_) => Err(format!(
            "ffmpeg took longer than {}s over it",
            FFMPEG_LIMIT.as_secs()
        )),
    }
}

/// The last thing ffmpeg said, short, for a note.
fn last_words(stderr: &str) -> String {
    stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("no output")
        .trim()
        .chars()
        .take(200)
        .collect()
}

/// How long the video runs, from what ffmpeg says when it opens it.
fn duration_of(stderr: &str) -> Option<f64> {
    let rest = stderr.split("Duration: ").nth(1)?;
    let stamp = rest.split(',').next()?.trim();
    let mut parts = stamp.split(':').map(|p| p.parse::<f64>().ok());
    let (h, m, s) = (parts.next()??, parts.next()??, parts.next()??);
    Some(h * 3600.0 + m * 60.0 + s)
}

/// Take frames from `video` into `out` (emptied first): about one a second,
/// spread over a longer video, plus where the picture changes, thinned to
/// [`MAX_FRAMES`] keeping the first and the last. Each file is named by its
/// moment. The error is a plain sentence about the video.
pub async fn frames(video: &Path, out: &Path) -> Result<(Vec<Frame>, Option<f64>), String> {
    let input = video.to_string_lossy().into_owned();
    // Opening it alone (no output) says how long it runs and what it holds.
    let (_, _, opened) = run(&["-nostdin", "-hide_banner", "-i", &input]).await?;
    if !opened.contains("Video:") {
        return Err(if opened.contains("Audio:") {
            "it has sound but no picture".to_string()
        } else {
            format!("ffmpeg could not open it ({})", last_words(&opened))
        });
    }
    let duration = duration_of(&opened);
    let step = duration.map_or(1.0, |d| (d / MAX_FRAMES as f64).max(1.0));

    let _ = std::fs::remove_dir_all(out);
    std::fs::create_dir_all(out).map_err(|e| format!("its frames could not be written ({e})"))?;
    // Sampled at 4 a second first, so a change is caught within a quarter
    // second and the change detector compares a few frames, not every one.
    let filter = format!(
        "fps=4,select='isnan(prev_selected_t)+gte(t-prev_selected_t\\,{:.2})+gt(scene\\,0.3)*gte(t-prev_selected_t\\,0.5)',\
         scale='if(gt(iw\\,ih)\\,min({LONG_SIDE}\\,iw)\\,-2)':'if(gt(iw\\,ih)\\,-2\\,min({LONG_SIDE}\\,ih))',showinfo",
        step - 0.01
    );
    let pattern = out.join("taken-%03d.jpg").to_string_lossy().into_owned();
    let taken = MAX_TAKEN.to_string();
    let (ok, _, said) = run(&[
        "-nostdin",
        "-hide_banner",
        "-y",
        "-i",
        &input,
        "-an",
        "-vf",
        &filter,
        "-vsync",
        "vfr",
        "-frames:v",
        &taken,
        "-q:v",
        "4",
        &pattern,
    ])
    .await?;
    // showinfo says each frame's moment, in the order they were written.
    let moments: Vec<f64> = said
        .lines()
        .filter(|l| l.contains("Parsed_showinfo"))
        .filter_map(|l| {
            l.split("pts_time:")
                .nth(1)?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
        .collect();
    let mut frames: Vec<Frame> = moments
        .iter()
        .enumerate()
        .map(|(n, at)| Frame {
            at: *at,
            path: out.join(format!("taken-{:03}.jpg", n + 1)),
        })
        .filter(|f| f.path.is_file())
        .collect();
    if frames.is_empty() {
        return Err(if ok {
            "no frames could be taken from it".to_string()
        } else {
            format!("ffmpeg stopped ({})", last_words(&said))
        });
    }

    // Thin to the most the helper watches, evenly, first and last kept.
    if frames.len() > MAX_FRAMES {
        let n = frames.len();
        let keep: std::collections::BTreeSet<usize> = (0..MAX_FRAMES)
            .map(|i| (i * (n - 1) + (MAX_FRAMES - 1) / 2) / (MAX_FRAMES - 1))
            .collect();
        frames = frames
            .into_iter()
            .enumerate()
            .filter_map(|(i, f)| {
                if keep.contains(&i) {
                    Some(f)
                } else {
                    let _ = std::fs::remove_file(&f.path);
                    None
                }
            })
            .collect();
    }
    // Named by their moment, so a frame read again says when it was.
    for f in &mut frames {
        let named = out.join(format!("at-{}.jpg", clock(f.at).replace(':', "m")));
        if std::fs::rename(&f.path, &named).is_ok() {
            f.path = named;
        }
    }
    Ok((frames, duration))
}

/// The video's sound track, for the transcriber, as 16 kHz mono FLAC: None
/// when it has no sound or only silence. The error is a plain sentence.
pub async fn sound(video: &Path) -> Result<Option<Vec<u8>>, String> {
    let input = video.to_string_lossy().into_owned();
    let (ok, flac, said) = run(&[
        "-nostdin",
        "-hide_banner",
        "-i",
        &input,
        "-vn",
        "-ac",
        "1",
        "-ar",
        "16000",
        "-af",
        "volumedetect",
        "-c:a",
        "flac",
        "-f",
        "flac",
        "pipe:1",
    ])
    .await?;
    if !said.contains("Audio:") {
        return Ok(None);
    }
    if !ok {
        return Err(format!(
            "ffmpeg could not take its sound ({})",
            last_words(&said)
        ));
    }
    let loudest = said
        .split("max_volume: ")
        .nth(1)
        .and_then(|r| r.split_whitespace().next()?.parse::<f64>().ok());
    if loudest.is_some_and(|db| db < SILENT_DB) || flac.is_empty() {
        return Ok(None);
    }
    Ok(Some(flac))
}

/// What the conversation holds for one attached video: the vision helper's
/// reading of its frames in order, the frames' files, or — when it could
/// not be read — that it is there and why not. Never longer than
/// [`crate::sidecar::READING_LIMIT`]: the owner's message waits on it.
pub async fn read(
    trace: ai::RequestTrace,
    provider: Option<&dyn ai::Provider>,
    video: Option<&Path>,
    file_id: &str,
    reference: &str,
    context: &str,
) -> String {
    let Some(video) = video else {
        return crate::sidecar::video_note(
            reference,
            None,
            &[],
            None,
            Some("its file is not on this computer"),
        );
    };
    let Some(out) = frames_dir(file_id) else {
        return crate::sidecar::video_note(
            reference,
            None,
            &[],
            None,
            Some("its frames had nowhere to be written"),
        );
    };
    let watched = async {
        let (frames, duration) = match frames(video, &out).await {
            Ok(taken) => taken,
            Err(why) => return crate::sidecar::video_note(reference, None, &[], None, Some(&why)),
        };
        let Some(provider) = provider else {
            return crate::sidecar::video_note(
                reference,
                duration,
                &frames,
                None,
                Some("no vision helper is set up"),
            );
        };
        let pictures: Vec<(f64, Picture)> = frames
            .iter()
            .filter_map(|f| Some((f.at, Picture::from_bytes(&std::fs::read(&f.path).ok()?)?)))
            .collect();
        let reading = crate::sidecar::watch(trace, provider, &pictures, duration, context).await;
        crate::sidecar::video_note(reference, duration, &frames, reading.as_deref(), None)
    };
    match tokio::time::timeout(crate::sidecar::READING_LIMIT, watched).await {
        Ok(note) => note,
        Err(_) => crate::sidecar::video_note(
            reference,
            None,
            &[],
            None,
            Some(&format!(
                "it was not read within {}s; any frames taken are in {}",
                crate::sidecar::READING_LIMIT.as_secs(),
                out.display()
            )),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        /// Set by a test to act as a machine with no ffmpeg.
        pub(super) static NO_FFMPEG: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    #[test]
    fn a_video_is_known_by_its_type_or_its_container() {
        assert!(is_video("ScreenRecording.mp4", "video/mp4"));
        assert!(is_video("clip.MOV", "application/octet-stream"));
        assert!(is_video("clip.mp4", ""));
        assert!(
            !is_video("memo.mp4", "audio/mp4"),
            "a sound-only mp4 is audio"
        );
        assert!(!is_video("memo.m4a", "audio/mp4"));
        assert!(!is_video("photo.jpg", "image/jpeg"));
    }

    #[test]
    fn moments_read_as_people_say_them() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(3.0), "0:03");
        assert_eq!(clock(62.5), "1:02.5");
        assert_eq!(
            duration_of("  Duration: 00:01:02.50, start: 0.000000"),
            Some(62.5)
        );
        assert_eq!(duration_of("  Duration: N/A, start"), None);
    }

    /// A short test video, made with ffmpeg: a moving test pattern with a
    /// tone, or silence when `tone` is false. None without ffmpeg.
    pub(crate) fn make_video(dir: &Path, secs: u32, tone: bool) -> Option<PathBuf> {
        let ffmpeg = which::which("ffmpeg").ok()?;
        let path = dir.join("recording.mp4");
        let picture = format!("testsrc=duration={secs}:size=320x240:rate=10");
        let sound = if tone {
            format!("sine=frequency=440:duration={secs}")
        } else {
            format!("anullsrc=r=44100:cl=mono:d={secs}")
        };
        let made = command::new::<std::process::Command>(&ffmpeg, command::Console::Hidden)
            .args([
                "-y",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                &picture,
                "-f",
                "lavfi",
                "-i",
                &sound,
            ])
            .args([
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&path)
            .status()
            .ok()?;
        made.success().then_some(path)
    }

    /// Frames come out in order, about one a second, named by their moment,
    /// never more than the helper watches.
    #[tokio::test]
    async fn frames_are_taken_in_order_and_capped() {
        let dir = tempfile::tempdir().unwrap();
        let Some(video) = make_video(dir.path(), 30, true) else {
            assert!(ffmpeg().unwrap_err().contains("ffmpeg is not installed"));
            return;
        };
        let out = dir.path().join("frames");
        let (frames, duration) = frames(&video, &out).await.unwrap();
        assert!((duration.unwrap() - 30.0).abs() < 0.5);
        assert!(
            frames.len() >= 8 && frames.len() <= MAX_FRAMES,
            "{} frames",
            frames.len()
        );
        assert!(frames.windows(2).all(|w| w[0].at < w[1].at), "in order");
        assert_eq!(frames[0].at, 0.0);
        assert!(frames.iter().all(|f| {
            f.path.is_file()
                && f.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("at-")
        }));
        assert_eq!(
            std::fs::read_dir(&out).unwrap().count(),
            frames.len(),
            "the thinned frames are gone"
        );
    }

    /// A sound track is handed over; silence is not.
    #[tokio::test]
    async fn sound_is_taken_and_silence_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let Some(loud) = make_video(dir.path(), 2, true) else {
            return;
        };
        assert!(
            sound(&loud)
                .await
                .unwrap()
                .is_some_and(|flac| flac.starts_with(b"fLaC"))
        );
        let quiet = dir.path().join("quiet");
        std::fs::create_dir_all(&quiet).unwrap();
        let quiet = make_video(&quiet, 2, false).unwrap();
        assert!(sound(&quiet).await.unwrap().is_none());
    }

    /// A file ffmpeg can't open is said plainly.
    #[tokio::test]
    async fn a_broken_video_is_said_plainly() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.mp4");
        std::fs::write(&bad, b"not a video").unwrap();
        let err = frames(&bad, &dir.path().join("f")).await.unwrap_err();
        assert!(err.contains("ffmpeg"), "{err}");
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    /// What the scripted vision helper writes about every video.
    const READING: &str = "TIMELINE: at 0:01 the user taps START; nothing changes.";

    /// A vision model that keeps what it was asked and answers [`READING`].
    #[derive(Default)]
    struct Seeing(std::sync::Mutex<Vec<ai::ChatRequest>>);

    #[async_trait::async_trait]
    impl ai::Provider for Seeing {
        fn id(&self) -> &str {
            "seeing"
        }
        fn supports_vision(&self) -> bool {
            true
        }
        async fn stream(
            &self,
            req: &ai::ChatRequest,
        ) -> Result<ai::EventReceiver, ai::ProviderError> {
            self.0.lock().unwrap().push(req.clone());
            let (tx, rx) = tokio::sync::mpsc::channel(2);
            tx.send(ai::StreamEvent::text(READING)).await.unwrap();
            tx.send(ai::StreamEvent::done()).await.unwrap();
            Ok(rx)
        }
    }

    /// The owner's screen recording reaches the helper as frames in order,
    /// each with its moment, with his words beside them; the note carries
    /// the reading and names each frame's file, in order, so one can be
    /// looked at again.
    #[test]
    fn a_video_is_read_as_frames_in_order() {
        crate::test_home::with_home(|home| {
            rt().block_on(async {
                let Some(video) = make_video(home, 5, true) else {
                    return;
                };
                let seeing = Seeing::default();
                let note = read(
                    ai::RequestTrace::new("video_read"),
                    Some(&seeing),
                    Some(&video),
                    "file-1",
                    "recording.mp4",
                    "The owner attached this video to their message: \"doesn't work\"",
                )
                .await;
                let asked = seeing.0.lock().unwrap();
                assert_eq!(asked.len(), 1, "one watch for the whole video");
                let ask = &asked[0].messages[0];
                let shown = ask
                    .images
                    .as_ref()
                    .expect("the helper sees the frames")
                    .len();
                assert!((5..=MAX_FRAMES).contains(&shown), "{shown} frames");
                assert!(ask.content.contains("doesn't work"), "{}", ask.content);
                assert!(
                    ask.content.contains("image 1: at 0:00")
                        && ask.content.contains("image 2: at 0:01"),
                    "{}",
                    ask.content
                );
                assert!(asked[0].system.contains("TIMELINE"));

                assert!(
                    note.starts_with("[Video: recording.mp4, 0:05 long.") && note.contains(READING),
                    "{note}"
                );
                let frames: Vec<&str> = note.lines().filter(|l| l.contains("/at-")).collect();
                assert_eq!(frames.len(), shown, "{note}");
                assert!(
                    frames[0].starts_with("0:00 ") && frames[1].starts_with("0:01 "),
                    "{note}"
                );
                for line in frames {
                    let path = line.split_once(' ').unwrap().1.trim_end_matches(']');
                    assert!(Path::new(path).is_file(), "frame file {path}");
                }
            })
        });
    }

    /// No ffmpeg on the bot: the note says so plainly, and nothing waits.
    #[test]
    fn no_ffmpeg_is_said_plainly() {
        crate::test_home::with_home(|home| {
            rt().block_on(async {
                NO_FFMPEG.with(|n| n.set(true));
                let video = home.join("recording.mp4");
                std::fs::write(&video, b"not looked at").unwrap();
                let seeing = Seeing::default();
                let note = read(
                    ai::RequestTrace::new("video_read"),
                    Some(&seeing),
                    Some(&video),
                    "file-2",
                    "recording.mp4",
                    "",
                )
                .await;
                let deaf = sound(&video).await;
                NO_FFMPEG.with(|n| n.set(false));
                assert!(
                    note.contains(
                        "Could not read this video: ffmpeg is not installed on this computer"
                    ),
                    "{note}"
                );
                assert!(
                    seeing.0.lock().unwrap().is_empty(),
                    "nothing to show the helper"
                );
                assert_eq!(
                    deaf.unwrap_err(),
                    "ffmpeg is not installed on this computer"
                );
            })
        });
    }
}
