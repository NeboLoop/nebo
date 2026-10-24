//! The vision helper: the one way an image reaches an employee.
//!
//! No image ever enters an employee's conversation. A file it reads, a
//! screenshot a tool takes, a picture the owner attaches: each is handed to
//! one model call on the turn's intelligence pack's Vision model ([`Eyes`])
//! that looks at it and answers in text. The
//! conversation holds that reading and the image's reference; a question
//! about the same image later is another look (`read_file` with `question`),
//! answered in text again. A summary keeps the reading like any other text,
//! and a checkpoint never meets an image, so one large picture can't push
//! the conversation past its window and into a summarize-and-re-read loop
//! (2026-09-30: a landscape plan PNG read, summarized away, read again, a
//! dozen times, and the plans were never drawn).

use std::sync::Arc;

use ai::image_norm::Picture;
use ai::{ChatRequest, Message, Provider, StreamEventType};
use tracing::{debug, warn};

const READER_SYSTEM: &str = "You are the eyes of an AI employee. Its own conversation never holds \
images: you look at this one for it, and what you write is all it will ever know of the image. Be \
thorough and exact.\n\n\
Write a structured reading in these parts:\n\
WHAT IT IS: the kind of image (photo, screenshot, drawing, plan, diagram, chart, document scan, \
receipt, ...) and its subject, in a sentence or two.\n\
TEXT: every piece of legible text, verbatim: titles, labels, notes, numbers, dimensions and \
measurements with their units, legends, scales, stamps. Say where each one is (\"north wall: \
24'-0\\\"\").\n\
LAYOUT: how the parts are arranged: what is where, what is next to, inside, above or connected to \
what, relative sizes, orientation, scale.\n\
DETAILS: anything else someone working from this image needs: counts, colors that carry meaning, \
symbols, marks, handwriting, anything unclear or cut off.\n\
SCREEN (only for a screenshot of a screen, an app or a web page): the app or page in focus, any \
blocker (sign-in, captcha, paywall, cookie banner, age gate, rate limit), and up to 8 things to act \
on, each as '<description> @ (<x>,<y>)' at its approximate center.\n\n\
When you are asked a question, answer it first and precisely, then give the reading. Report only \
what you can actually see and say plainly what is illegible. Never invent content. No preamble.";

const WATCHER_SYSTEM: &str = "You are the eyes of an AI employee. Its own conversation never holds \
images or video: you watch this video for it, as frames taken from it in order, each with the moment \
it was taken, and what you write is all it will ever know of the video. Be thorough and exact.\n\n\
Write a structured reading in these parts:\n\
WHAT IT IS: the kind of video (a screen recording of an app or a web page, camera footage, ...) and \
its subject, in a sentence or two.\n\
TIMELINE: what happens, in order, by moment: \"at 0:03 the user taps START; nothing changes; at 0:06 \
...\". Say what changed from one frame to the next, and what did not change where a change was \
expected (a tap with no response, a spinner that never ends, an error that appears).\n\
TEXT: the legible text that matters, verbatim, with when it is on screen.\n\
OUTCOME: how it ends, and what the person who made it seems to be showing (a bug, a step, a result).\n\n\
You see only the frames: say \"between 0:03 and 0:04\" rather than inventing what happened between \
them. Report only what you can actually see and say plainly what is illegible. Never invent content. \
No preamble.";

/// Most tokens one reading takes. A reasoning model counts its hidden
/// reasoning in the same cap, so this holds a full reading of a dense
/// screenshot (all of its text verbatim can run past 1,500 tokens) and
/// the little reasoning [`READING_EFFORT`] still spends. Only the tokens
/// written are charged.
const READING_TOKENS: i32 = 4_000;

/// Most tokens one video's reading takes: a timeline over many frames, and
/// the same room for reasoning.
const WATCHING_TOKENS: i32 = 6_000;

/// The reasoning a look or a watch asks for: the lowest a request takes.
/// Describing what is in a picture needs little thought, and a reasoning
/// model left at its default thinks through the whole output cap and writes
/// nothing (2026-10-24: two screenshots read on a reasoning model came back
/// empty, each at exactly the cap).
const READING_EFFORT: types::packs::ProviderEffort = types::packs::ProviderEffort::Low;

/// The longest one look may take, from asking to the last word. The owner's
/// message waits on the readings of its pictures before it is stored and
/// its turn starts (`harness::turn::read_pictures`), so a helper that never
/// answers would hold the employee at "Working…" for good. Past this the
/// look is given up and the note says the image could not be read; the
/// pictures on one message are read side by side, so this bounds the whole
/// step too. A video's whole reading, its frames taken and watched, has the
/// same bound (`video::read`).
pub const READING_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

/// Who reads an image: the provider and the model name it is sent (empty:
/// the provider's own default), from the turn's pack's Vision slot
/// (`Harness::eyes`).
#[derive(Clone)]
pub struct Eyes {
    pub provider: Arc<dyn Provider>,
    pub model: String,
}

/// One look at `picture`: the helper's reading, in text. `context` says
/// where the image came from; `question`, when the employee asked one, is
/// answered first. None when the helper could not answer.
pub async fn look(
    trace: ai::RequestTrace,
    eyes: &Eyes,
    picture: &Picture,
    context: &str,
    question: Option<&str>,
) -> Option<String> {
    // A provider that never puts images on the wire (a CLI wrapper, a
    // local model) would answer as if it had seen nothing.
    if !eyes.provider.supports_vision() {
        return None;
    }
    let mut ask = format!("{context}\nThe image is {}×{} pixels.", picture.width, picture.height);
    if let Some(q) = question.map(str::trim).filter(|q| !q.is_empty()) {
        ask.push_str(&format!("\nQuestion: {q}"));
    }
    read(trace, eyes, READER_SYSTEM, ask, vec![picture.image.clone()], READING_TOKENS).await
}

/// One watch of a video, as frames in order, each with the moment it was
/// taken (seconds from the start): the helper's reading of what happens, in
/// text. `context` says where the video came from. None when the helper
/// could not answer.
pub async fn watch(
    trace: ai::RequestTrace,
    eyes: &Eyes,
    frames: &[(f64, Picture)],
    duration: Option<f64>,
    context: &str,
) -> Option<String> {
    if !eyes.provider.supports_vision() || frames.is_empty() {
        return None;
    }
    let long = duration.map(|d| format!(", {} long", crate::video::clock(d))).unwrap_or_default();
    let mut ask = format!("{context}\nThese are {} frames from the video{long}, in order:", frames.len());
    for (n, (at, picture)) in frames.iter().enumerate() {
        ask.push_str(&format!("\nimage {}: at {} ({}×{} px)", n + 1, crate::video::clock(*at), picture.width, picture.height));
    }
    let images = frames.iter().map(|(_, p)| p.image.clone()).collect();
    read(trace, eyes, WATCHER_SYSTEM, ask, images, WATCHING_TOKENS).await
}

/// One call to the helper: `ask` and `images` under `system`, answered in
/// text, given up past [`READING_LIMIT`].
async fn read(
    trace: ai::RequestTrace,
    eyes: &Eyes,
    system: &str,
    ask: String,
    images: Vec<ai::ImageContent>,
    max_tokens: i32,
) -> Option<String> {
    let req = ChatRequest {
        tool_credential: None,
        caller_waits_for_balance: false,
        chat_id: String::new(),
        ask_channels: None,
        permission_mode: None,
        linked_context: None,
        tool_choice: Default::default(),
        messages: vec![Message {
            role: "user".to_string(),
            content: ask,
            images: Some(images),
            ..Default::default()
        }],
        tools: vec![],
        max_tokens,
        temperature: 0.0,
        system: system.to_string(),
        model: eyes.model.clone(),
        enable_thinking: false,
        effort: Some(READING_EFFORT),
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
        trace,
    };

    let reading = async {
        let mut rx = match eyes.provider.stream(&req).await {
            Ok(rx) => rx,
            Err(e) => {
                debug!("vision reading failed: {e}");
                return None;
            }
        };
        let mut text = String::new();
        let mut usage = None;
        let mut stop_reason = None;
        while let Some(event) = rx.recv().await {
            match event.event_type {
                StreamEventType::Text => text.push_str(&event.text),
                StreamEventType::Usage => usage = event.usage,
                StreamEventType::Done => {
                    stop_reason = event.stop_reason;
                    break;
                }
                StreamEventType::Error => break,
                _ => {}
            }
        }
        let text = text.trim();
        if text.is_empty() && crate::harness::model_call::hit_output_limit(stop_reason.as_deref()) {
            warn!(
                provider = eyes.provider.id(),
                model = %eyes.model,
                max_tokens,
                output_tokens = usage.as_ref().map(|u| u.output_tokens),
                input_tokens = usage.as_ref().map(|u| u.input_tokens),
                "vision reading came back empty: the output cap ran out before any text"
            );
        }
        (!text.is_empty()).then(|| text.to_string())
    };
    match tokio::time::timeout(READING_LIMIT, reading).await {
        Ok(reading) => reading,
        Err(_) => {
            warn!("vision reading gave no answer within {READING_LIMIT:?}; the note says it could not be read");
            None
        }
    }
}

/// What the conversation holds for one image: its reference and size, the
/// helper's reading, and how to look again. With no reading, it says an
/// image is there and could not be read, so the model never claims there
/// was none or guesses at what it shows.
pub fn note(reference: &str, size: Option<(u32, u32)>, reading: Option<&str>) -> String {
    let size = size.map(|(w, h)| format!(", {w}×{h} px")).unwrap_or_default();
    match reading {
        Some(reading) => format!(
            "[Image: {reference}{size}. A vision helper looked at it for you: this reading is what you have of \
             it. To check a detail, look again with read_file(path, question: \"...\").]\n{reading}"
        ),
        None => format!(
            "[Image: {reference}{size}. The vision helper could not read it, so you do not know what it shows. \
             Do not guess or describe what it might be. Tell the owner once that you could not read it and ask \
             them to send it again or paste its text; don't claim there was no image.]"
        ),
    }
}

/// What the conversation holds for one video: its reference and length,
/// the helper's reading, and each frame's file with its moment, so a frame
/// can be looked at again. With no reading, it says a video is there, could
/// not be read and why (`problem`), so the model never guesses at it.
pub fn video_note(
    reference: &str,
    duration: Option<f64>,
    frames: &[crate::video::Frame],
    reading: Option<&str>,
    problem: Option<&str>,
) -> String {
    let long = duration.map(|d| format!(", {} long", crate::video::clock(d))).unwrap_or_default();
    let listed = if frames.is_empty() {
        String::new()
    } else {
        let lines: Vec<String> =
            frames.iter().map(|f| format!("{} {}", crate::video::clock(f.at), f.path.display())).collect();
        format!(
            " Its frames are files; to check a moment, look at one with read_file(path, question: \"...\"):\n{}",
            lines.join("\n")
        )
    };
    match reading {
        Some(reading) => format!(
            "[Video: {reference}{long}. A vision helper watched it for you as {} frames in order: this reading is \
             what you have of it.{listed}]\n{reading}",
            frames.len()
        ),
        None => {
            let why = problem.unwrap_or("the vision helper gave no answer");
            format!(
                "[Video: {reference}{long}. Could not read this video: {why}. Say so plainly if the work depends on \
                 it; don't guess at what it shows.{listed}]"
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The note names the image and its size, carries the reading, and
    /// says how to look again; without a reading it still says an image is
    /// there.
    #[test]
    fn the_note_names_the_image_and_says_how_to_look_again() {
        let n = note("/plans/site.png", Some((4000, 3000)), Some("TEXT: DECK 12'x16'"));
        assert!(n.contains("/plans/site.png, 4000×3000 px") && n.contains("DECK 12'x16'") && n.contains("question"));
        let blind = note("/plans/site.png", None, None);
        assert!(blind.contains("could not read it") && blind.contains("don't claim there was no image"));
        assert!(blind.contains("you do not know what it shows") && blind.contains("Do not guess"));
        assert!(blind.contains("send it again or paste its text"));
    }

    /// A vision model that takes the request and never answers.
    struct Silent;

    #[async_trait::async_trait]
    impl Provider for Silent {
        fn id(&self) -> &str {
            "silent"
        }
        fn supports_vision(&self) -> bool {
            true
        }
        async fn stream(&self, _req: &ChatRequest) -> Result<ai::EventReceiver, ai::ProviderError> {
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            // Held open and never written: the answer that never comes.
            std::mem::forget(tx);
            Ok(rx)
        }
    }

    fn silent() -> Eyes {
        Eyes { provider: std::sync::Arc::new(Silent), model: String::new() }
    }

    /// A vision model that keeps each request and answers at once.
    #[derive(Default)]
    struct Kept(std::sync::Mutex<Vec<ChatRequest>>);

    #[async_trait::async_trait]
    impl Provider for Kept {
        fn id(&self) -> &str {
            "kept"
        }
        fn supports_vision(&self) -> bool {
            true
        }
        async fn stream(&self, req: &ChatRequest) -> Result<ai::EventReceiver, ai::ProviderError> {
            self.0.lock().unwrap().push(req.clone());
            let (tx, rx) = tokio::sync::mpsc::channel(2);
            tx.send(ai::StreamEvent::text("seen")).await.unwrap();
            tx.send(ai::StreamEvent::done()).await.unwrap();
            Ok(rx)
        }
    }

    /// Every look and watch names its most tokens, so the gateway holds
    /// that much of the balance and not its large default, and asks for the
    /// lowest reasoning, so a reasoning model spends the tokens on the
    /// reading and does not think through the whole cap.
    #[tokio::test]
    async fn a_look_and_a_watch_name_their_most_tokens_and_the_lowest_reasoning() {
        let picture = Picture {
            image: ai::ImageContent { media_type: "image/png".into(), data: String::new() },
            width: 10,
            height: 10,
        };
        let kept = std::sync::Arc::new(Kept::default());
        let eyes = Eyes { provider: kept.clone(), model: String::new() };
        look(ai::RequestTrace::new("image_read"), &eyes, &picture, "attached", None).await;
        watch(ai::RequestTrace::new("video_read"), &eyes, &[(0.0, picture.clone())], Some(1.0), "attached").await;
        let sent: Vec<(String, i32, Option<types::packs::ProviderEffort>)> =
            kept.0.lock().unwrap().iter().map(|r| (r.trace.purpose.to_string(), r.max_tokens, r.effort)).collect();
        let low = Some(types::packs::ProviderEffort::Low);
        assert_eq!(sent, vec![("image_read".to_string(), 4_000, low), ("video_read".to_string(), 6_000, low)]);
    }

    /// A helper that never answers gives up within the limit, and the
    /// owner's message goes on with a note that the image could not be read.
    #[tokio::test(start_paused = true)]
    async fn a_look_that_never_answers_gives_up_within_the_limit() {
        let picture = Picture {
            image: ai::ImageContent { media_type: "image/png".into(), data: String::new() },
            width: 10,
            height: 10,
        };
        let started = tokio::time::Instant::now();
        // Bounded from outside too, so a look with no limit fails here rather than hanging.
        let reading = tokio::time::timeout(
            READING_LIMIT * 2,
            look(ai::RequestTrace::new("image_read"), &silent(), &picture, "attached", None),
        )
        .await
        .expect("the look never gave up");
        assert!(reading.is_none());
        assert!(started.elapsed() <= READING_LIMIT + std::time::Duration::from_secs(1));
        assert!(note("photo.png", Some((10, 10)), reading.as_deref()).contains("could not read it"));
    }

    /// A helper that never answers a video gives up within the same limit:
    /// the owner's message goes on, its note says the video could not be
    /// read, and the frames are still named so one can be looked at.
    #[tokio::test(start_paused = true)]
    async fn a_watch_that_never_answers_gives_up_and_the_message_goes_on() {
        let picture = Picture {
            image: ai::ImageContent { media_type: "image/jpeg".into(), data: String::new() },
            width: 10,
            height: 10,
        };
        let started = tokio::time::Instant::now();
        let reading = tokio::time::timeout(
            READING_LIMIT * 2,
            watch(ai::RequestTrace::new("video_read"), &silent(), &[(0.0, picture.clone()), (1.0, picture)], Some(2.0), "attached"),
        )
        .await
        .expect("the watch never gave up");
        assert!(reading.is_none());
        assert!(started.elapsed() <= READING_LIMIT + std::time::Duration::from_secs(1));
        let frames = [crate::video::Frame { at: 1.0, path: "/frames/at-0m01.jpg".into() }];
        let n = video_note("recording.mp4", Some(2.0), &frames, reading.as_deref(), None);
        assert!(n.contains("Could not read this video") && n.contains("don't guess"), "{n}");
        assert!(n.contains("0:01 /frames/at-0m01.jpg"), "{n}");
    }
}
