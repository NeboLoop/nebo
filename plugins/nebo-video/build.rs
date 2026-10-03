//! Embeds the static ffmpeg + ffprobe built by `scripts/build-ffmpeg.sh`.
//!
//! The plugin must work on a machine that has never heard of ffmpeg, so a
//! build without the vendored executables is an error, not a fallback.

use std::env;
use std::fs;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

fn main() {
    let target = env::var("TARGET").expect("cargo sets TARGET");
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let exe = if target.contains("windows") { ".exe" } else { "" };

    println!("cargo:rerun-if-env-changed=NEBO_VIDEO_FFMPEG_DIR");
    let dir = env::var_os("NEBO_VIDEO_FFMPEG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("vendor").join(&target));

    let font = manifest.join("assets").join("Inter-Regular.ttf");
    let mut hasher = Sha256::new();
    for (key, path) in [
        ("NEBO_VIDEO_FFMPEG", dir.join(format!("ffmpeg{exe}"))),
        ("NEBO_VIDEO_FFPROBE", dir.join(format!("ffprobe{exe}"))),
        ("NEBO_VIDEO_FONT", font),
    ] {
        let bytes = fs::read(&path).unwrap_or_else(|_| {
            panic!(
                "missing {}\nbuild it first: scripts/build-ffmpeg.sh {target}\n\
                 (or point NEBO_VIDEO_FFMPEG_DIR at a directory holding ffmpeg{exe} and ffprobe{exe})",
                path.display()
            )
        });
        hasher.update(&bytes);
        println!("cargo:rerun-if-changed={}", path.display());
        println!("cargo:rustc-env={key}={}", path.display());
    }

    // Names the extraction directory, so a plugin upgrade never runs an older
    // ffmpeg left on disk by a previous version.
    let digest = hasher.finalize();
    let hash: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    println!("cargo:rustc-env=NEBO_VIDEO_ASSET_HASH={hash}");
}
