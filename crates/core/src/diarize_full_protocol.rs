//! Versioned anonymous-speaker protocol shared by the host and unpublished helper.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

pub const PROTOCOL: u32 = 1;
pub const ENGINE: &str = "speakrs-af8013f-cpu-v1";
pub const MODEL_REVISION: &str = "67f963b6a1d94fcd4678948237b3cc8bd69bf1d8";
pub const PROFILE_NAMESPACE: &str = "speakrs_af8013f_cpu_67f963b6_anonymous_v1";
pub const MAX_SAMPLES: usize = 16_000 * 3600;
pub const MAX_SEGMENTS: usize = 100_000;
pub const OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;
pub const MEMORY_BYTES: u64 = 4 * 1024 * 1024 * 1024;
// Closed diagnostics: never reflect native errors, paths or audio through logs.
pub const FAILURE_CLASSES: &[&str] = &[
    "model_load",
    "inference",
    "unsupported_mode",
    "segmentation",
    "powerset",
    "plda",
    "reconstruct",
    "queue",
    "invariant",
    "worker_panic",
    "worker_spawn",
    "backend",
    "other",
    "unclassified",
];
pub const MODEL_FILES: &[(&str, &str)] = &[
    (
        "segmentation-3.0.safetensors",
        "88e7883cf56f679170e162419efddf970f929915281ebce80e351c42b5a7115d",
    ),
    (
        "wespeaker-multimask-tail.safetensors",
        "b97b090ceebcf10924732ad4552a2d867d76d38dcc1409a13687a6ba71ff1ef3",
    ),
    (
        "plda_lda.npy",
        "e20c9b012bebd1aabda5a38a127e63a43cf35debdc502715fc143e2fb6bc3c4b",
    ),
    (
        "plda_tr.npy",
        "e700b68cb319de3fafb5fa093eb9222c23c447084741f8d3a533640d425510ee",
    ),
    (
        "plda_mu.npy",
        "d286d48acf99bbc1ed1502fed0a3e361ae5626ce1870c8be9f7397c5e47886c6",
    ),
    (
        "plda_psi.npy",
        "d7128c9ed2f28a9781971805131129f077c04f948e2df12e52dcdb99f2b4e5f5",
    ),
    (
        "plda_mean1.npy",
        "e424c0c352182aa8e0f555dec1f3b30e29a20b9ed6b25d339f112af92e51e36f",
    ),
    (
        "plda_mean2.npy",
        "6f6fb708a2037197b5b84ffeaa8f140cb878088fbecd6ab042ad26a7691bd2cf",
    ),
    (
        "wespeaker-voxceleb-resnet34.min_num_samples.txt",
        "e4df891c484d7abb985dadf539fa1883a646dab6337af5cae4159c587b7050cc",
    ),
];

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    pub speaker: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub protocol: u32,
    pub engine: String,
    pub model_revision: String,
    pub sample_count: usize,
    pub audio_sha256: String,
    pub segments: Vec<Segment>,
}

pub fn verify_models(directory: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !directory.is_absolute() {
        return Err("full speaker models require an absolute directory".into());
    }
    for (name, expected) in MODEL_FILES {
        let path = directory.join(name);
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.len() > 40 * 1024 * 1024 {
            return Err("full speaker model is not a bounded regular file".into());
        }
        let mut file = std::fs::File::open(path)?.take(40 * 1024 * 1024 + 1);
        let mut hasher = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        if format!("{:x}", hasher.finalize()) != *expected {
            return Err("full speaker model hash differs from the pinned bundle".into());
        }
    }
    Ok(())
}
