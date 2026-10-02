//! The vision helper: the one way an image reaches an employee.
//!
//! No image ever enters an employee's conversation. A file it reads, a
//! screenshot a tool takes, a picture the owner attaches: each is handed to
//! one model call on the sidecar model (the pick every side call makes,
//! through the same provider) that looks at it and answers in text. The
//! conversation holds that reading and the image's reference; a question
//! about the same image later is another look (`read_file` with `question`),
//! answered in text again. A summary keeps the reading like any other text,
//! and a checkpoint never meets an image, so one large picture can't push
//! the conversation past its window and into a summarize-and-re-read loop
//! (2026-09-30: a landscape plan PNG read, summarized away, read again, a
//! dozen times, and the plans were never drawn).

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

/// Most tokens one reading takes.
const READING_TOKENS: i32 = 1_500;

/// The longest one look may take, from asking to the last word. The owner's
/// message waits on the readings of its pictures before it is stored and
/// its turn starts (`harness::turn::read_pictures`), so a helper that never
/// answers would hold the employee at "Working…" for good. Past this the
/// look is given up and the note says the image could not be read; the
/// pictures on one message are read side by side, so this bounds the whole
/// step too.
pub const READING_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

/// Resolve the sidecar model for the provider that will serve the request.
/// Empty string lets the provider pick.
fn sidecar_model(provider: &str) -> String {
    config::ModelsConfig::load()
        .sidecar_model(provider)
        .unwrap_or_default()
}

/// One look at `picture`: the helper's reading, in text. `context` says
/// where the image came from; `question`, when the employee asked one, is
/// answered first. None when the helper could not answer.
pub async fn look(
    trace: ai::RequestTrace,
    provider: &dyn Provider,
    picture: &Picture,
    context: &str,
    question: Option<&str>,
) -> Option<String> {
    // A provider that never puts images on the wire (a CLI wrapper, a
    // local model) would answer as if it had seen nothing.
    if !provider.supports_vision() {
        return None;
    }
    let mut ask = format!("{context}\nThe image is {}×{} pixels.", picture.width, picture.height);
    if let Some(q) = question.map(str::trim).filter(|q| !q.is_empty()) {
        ask.push_str(&format!("\nQuestion: {q}"));
    }
    let req = ChatRequest {
        tool_credential: None,
        chat_id: String::new(),
        ask_channels: None,
        permission_mode: None,
        linked_context: None,
        tool_choice: Default::default(),
        messages: vec![Message {
            role: "user".to_string(),
            content: ask,
            images: Some(vec![picture.image.clone()]),
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: READING_TOKENS,
        temperature: 0.0,
        system: READER_SYSTEM.to_string(),
        model: sidecar_model(provider.id()),
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
        trace,
    };

    let reading = async {
        let mut rx = match provider.stream(&req).await {
            Ok(rx) => rx,
            Err(e) => {
                debug!("image reading failed: {e}");
                return None;
            }
        };
        let mut text = String::new();
        while let Some(event) = rx.recv().await {
            match event.event_type {
                StreamEventType::Text => text.push_str(&event.text),
                StreamEventType::Done | StreamEventType::Error => break,
                _ => {}
            }
        }
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    };
    match tokio::time::timeout(READING_LIMIT, reading).await {
        Ok(reading) => reading,
        Err(_) => {
            warn!("image reading gave no answer within {READING_LIMIT:?}; the note says it could not be read");
            None
        }
    }
}

/// What the conversation holds for one image: its reference and size, the
/// helper's reading, and how to look again. With no reading, it says an
/// image is there and could not be read, so the model never claims there
/// was none.
pub fn note(reference: &str, size: Option<(u32, u32)>, reading: Option<&str>) -> String {
    let size = size.map(|(w, h)| format!(", {w}×{h} px")).unwrap_or_default();
    match reading {
        Some(reading) => format!(
            "[Image: {reference}{size}. A vision helper looked at it for you: this reading is what you have of \
             it. To check a detail, look again with read_file(path, question: \"...\").]\n{reading}"
        ),
        None => format!(
            "[Image: {reference}{size}. The vision helper could not read it this time. Say so plainly if the \
             work depends on it; don't claim there was no image.]"
        ),
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
            look(ai::RequestTrace::new("image_read"), &Silent, &picture, "attached", None),
        )
        .await
        .expect("the look never gave up");
        assert!(reading.is_none());
        assert!(started.elapsed() <= READING_LIMIT + std::time::Duration::from_secs(1));
        assert!(note("photo.png", Some((10, 10)), reading.as_deref()).contains("could not read it"));
    }
}
