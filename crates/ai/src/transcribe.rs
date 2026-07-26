//! Speech-to-text for audio *files*.
//!
//! Distinct from `nebo-voice`, which is a live duplex stream — this is the
//! one-shot path for an audio attachment that arrives in a message. It speaks
//! the OpenAI `/audio/transcriptions` shape, which xAI, Groq, and the local
//! whisper servers all implement as well, so the endpoint is a base URL rather
//! than a provider enum.

use crate::ProviderError;

/// Providers reject anything larger, and the request would be a slow way to
/// find that out.
pub const MAX_AUDIO_BYTES: usize = 25 * 1024 * 1024;

/// File extensions the transcription endpoints accept.
pub const SUPPORTED_AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "m4a", "mp3", "mp4", "mpeg", "mpga", "oga", "ogg", "wav", "webm",
];

/// Whether this filename/MIME pair looks like audio we can transcribe.
pub fn is_transcribable(filename: &str, mime_type: &str) -> bool {
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext {
        Some(ext) if SUPPORTED_AUDIO_EXTENSIONS.contains(&ext.as_str()) => true,
        // A voice note recorded in the browser often arrives with a generic or
        // absent extension, so the declared type still gets a say.
        _ => mime_type.starts_with("audio/"),
    }
}

/// Transcribe an audio file. Returns the spoken text.
///
/// An empty transcript is returned as `Ok("")` — silence is a real answer, and
/// the caller says so in words rather than presenting nothing.
pub async fn transcribe(
    api_key: &str,
    base_url: &str,
    model: &str,
    filename: &str,
    bytes: Vec<u8>,
) -> Result<String, ProviderError> {
    if bytes.len() > MAX_AUDIO_BYTES {
        return Err(ProviderError::Request(format!(
            "audio file is {:.1} MB; the transcription limit is {} MB",
            bytes.len() as f64 / (1024.0 * 1024.0),
            MAX_AUDIO_BYTES / (1024 * 1024)
        )));
    }

    let part = reqwest::multipart::Part::bytes(bytes).file_name(filename.to_string());
    let form = reqwest::multipart::Form::new()
        .text("model", model.to_string())
        .text("response_format", "text")
        .part("file", part);

    let url = format!("{}/audio/transcriptions", base_url.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .post(&url)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| ProviderError::Request(e.to_string()))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| ProviderError::Request(e.to_string()))?;

    if !status.is_success() {
        return Err(match status.as_u16() {
            401 | 403 => ProviderError::Auth(body),
            429 => ProviderError::RateLimit,
            _ => ProviderError::Api {
                code: status.as_u16().to_string(),
                message: body,
                retryable: status.is_server_error(),
            },
        });
    }

    Ok(body.trim().to_string())
}
