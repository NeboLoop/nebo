//! Project JSON types (serde). Keep this schema stable — the agent writes
//! these projects and the Video Studio app will too.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct Project {
    pub output: OutputSpec,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct OutputSpec {
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default = "default_codec")]
    pub codec: String,
    #[serde(default = "default_vbr")]
    pub video_bitrate: String,
    #[serde(default = "default_abr")]
    pub audio_bitrate: String,
    #[serde(default = "default_pixfmt")]
    pub pixel_format: String,
}

fn default_width() -> u32 { 1920 }
fn default_height() -> u32 { 1080 }
fn default_fps() -> u32 { 30 }
fn default_codec() -> String { "h264".into() }
fn default_vbr() -> String { "5M".into() }
fn default_abr() -> String { "128k".into() }
fn default_pixfmt() -> String { "yuv420p".into() }

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Track {
    Video { clips: Vec<VideoClip> },
    Audio { clips: Vec<AudioClip> },
    Text  { clips: Vec<TextClip> },
}

#[derive(Debug, Deserialize, Serialize)]
pub struct VideoClip {
    pub source: String,
    pub start: f64,
    pub duration: f64,
    #[serde(default)]
    pub trim_start: f64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AudioClip {
    pub source: String,
    pub start: f64,
    #[serde(default)]
    pub trim_start: f64,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default)]
    pub filter: Option<String>,
}

fn default_volume() -> f32 { 1.0 }

#[derive(Debug, Deserialize, Serialize)]
pub struct TextClip {
    pub text: String,
    pub start: f64,
    pub duration: f64,
    #[serde(default = "default_text_size")]
    pub size: u32,
    #[serde(default = "default_text_x")]
    pub x: f32,
    #[serde(default = "default_text_y")]
    pub y: f32,
    #[serde(default = "default_text_color")]
    pub color: String,
}

fn default_text_size() -> u32 { 48 }
fn default_text_x() -> f32 { 0.5 }
fn default_text_y() -> f32 { 0.5 }
fn default_text_color() -> String { "white".into() }

impl Project {
    pub fn video_tracks(&self) -> impl Iterator<Item = &Vec<VideoClip>> {
        self.tracks.iter().filter_map(|t| match t {
            Track::Video { clips } => Some(clips),
            _ => None,
        })
    }
    pub fn audio_tracks(&self) -> impl Iterator<Item = &Vec<AudioClip>> {
        self.tracks.iter().filter_map(|t| match t {
            Track::Audio { clips } => Some(clips),
            _ => None,
        })
    }
    pub fn text_tracks(&self) -> impl Iterator<Item = &Vec<TextClip>> {
        self.tracks.iter().filter_map(|t| match t {
            Track::Text { clips } => Some(clips),
            _ => None,
        })
    }
}
