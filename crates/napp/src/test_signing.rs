//! Signed `.napp` fixtures for tests.
//!
//! A real `.napp` is signed by NeboAI, so no test can build one. This
//! signs with a throwaway key that `unwrap_napp_builtin` (and every reader
//! of a `.napp` on disk) also accepts — but only in a build with this
//! module, which exists under `cfg(test)` or the `test-signing` feature.
//! Crates enable that feature from their dev-dependencies alone; a shipped
//! build never compiles it.

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

/// A throwaway test key — never a release key.
fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

pub(crate) fn verifying_key() -> VerifyingKey {
    signing_key().verifying_key()
}

/// `entries` packed as tar.gz.
fn targz(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut targz = Vec::new();
    {
        let gz = flate2::write::GzEncoder::new(&mut targz, flate2::Compression::default());
        let mut builder = tar::Builder::new(gz);
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, name, *data)
                .expect("append tar entry");
        }
        builder
            .into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish gzip");
    }
    targz
}

/// `payload` wrapped in a `NAPP` envelope signed by the test key.
fn envelope(payload: &[u8]) -> Vec<u8> {
    let hash = Sha256::digest(payload);
    let mut signed = Vec::with_capacity(32 + payload.len());
    signed.extend_from_slice(&hash);
    signed.extend_from_slice(payload);
    let signature = signing_key().sign(&signed);

    let mut out = Vec::with_capacity(101 + payload.len());
    out.extend_from_slice(b"NAPP");
    out.push(0x01);
    out.extend_from_slice(&signature.to_bytes());
    out.extend_from_slice(&hash);
    out.extend_from_slice(payload);
    out
}

/// Build a free (unsealed) `.napp`: `entries` packed as tar.gz, wrapped in a
/// `NAPP` envelope signed by the test key — what the marketplace serves.
pub fn signed_napp(entries: &[(&str, &[u8])]) -> Vec<u8> {
    envelope(&targz(entries))
}

/// Build a sealed `.napp`: `entries` packed as tar.gz, encrypted with
/// `license_key`, wrapped in a `NAPP` envelope signed by the test key.
pub fn sealed_napp(entries: &[(&str, &[u8])], license_key: &[u8; 32]) -> Vec<u8> {
    let payload = crate::sealed::seal_payload(&targz(entries), license_key).expect("seal payload");
    envelope(&payload)
}
