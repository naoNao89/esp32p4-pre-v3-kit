//! Pinned `esp-hal` source preparation for pre-v3 ESP32-P4 silicon.
//!
//! The `cargo-esp32p4-pre-v3` binary is the normal entry point. This library
//! exposes the same inputs (`MANIFEST_JSON`, `PATCH_BYTES`, `Manifest`) and
//! the maintainer [`distribution`] packaging step for reuse and documentation.
use serde::Deserialize;

pub const MANIFEST_JSON: &str = include_str!("../patches/manifest.json");
pub const PATCH_BYTES: &[u8] = include_bytes!("../patches/esp32p4-pre-v3.patch");

#[derive(Deserialize, Debug)]
pub struct ManifestUpstream {
    pub repository: String,
    pub revision: String,
}

#[derive(Deserialize, Debug)]
pub struct ManifestPatch {
    #[serde(rename = "path")]
    pub _path: String,
    pub sha256: String,
}

#[derive(Deserialize, Debug)]
pub struct Manifest {
    pub upstream: ManifestUpstream,
    pub patch: ManifestPatch,
    pub cargo_patches: Vec<String>,
}

pub mod distribution;
