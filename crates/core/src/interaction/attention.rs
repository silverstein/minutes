//! Synchronized point-and-talk context and final-egress source attestation.

use serde::{Deserialize, Serialize};

use super::{next, text, Result, MAX_ITEMS, MAX_TEXT_BYTES};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// Platform-neutral opaque IDs, not process names or screen coordinates.
    pub application: String,
    pub window: String,
    pub artifact: Option<String>,
}

impl Target {
    pub fn validate(&self) -> Result<()> {
        text(&self.application)?;
        text(&self.window)?;
        if let Some(artifact) = &self.artifact {
            text(artifact)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Available,
    PermissionRequired,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureRequest {
    pub generation: u64,
    pub sequence: u64,
    pub target: Target,
    pub requested_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextPacket {
    pub request: CaptureRequest,
    pub observed_at_ms: u64,
    pub observed_target: Target,
    pub selected_text: Option<String>,
    /// An opaque reference to the exact image snapshot, not permission to read a path.
    pub image_ref: Option<String>,
}

/// A platform implements this at the edge. Unsupported means unsupported;
/// it must not fall back to broader capture or synthetic success.
pub trait ContextProvider {
    fn availability(&self) -> Availability;
    fn capture(&self, request: &CaptureRequest) -> Result<ContextPacket>;
}

/// One explicitly attended window. Not serializable; restarting revokes it.
#[derive(Debug, Default)]
pub struct AttentionSession {
    generation: u64,
    sequence: u64,
    target: Option<Target>,
    started_at_ms: u64,
    expires_at_ms: u64,
    pending: Option<CaptureRequest>,
}

impl AttentionSession {
    /// Trusted host entry point, called before an overlay steals focus.
    pub fn begin(&mut self, target: Target, now_ms: u64, ttl_ms: u64) -> Result<()> {
        target.validate()?;
        if ttl_ms == 0 || ttl_ms > 30 * 60 * 1000 {
            return Err("attention duration must be between 1 ms and 30 minutes".into());
        }
        let expires = now_ms.checked_add(ttl_ms).ok_or("clock overflow")?;
        self.generation = next(self.generation)?;
        self.target = Some(target);
        self.started_at_ms = now_ms;
        self.expires_at_ms = expires;
        self.pending = None;
        Ok(())
    }

    /// The host may refresh only the attended target. A newer request supersedes
    /// an older in-flight frame; mixing frame A with selection B is not allowed.
    pub fn request(&mut self, now_ms: u64) -> Result<CaptureRequest> {
        if now_ms < self.started_at_ms || now_ms >= self.expires_at_ms {
            return Err("attention expired or clock regressed".into());
        }
        let target = self.target.clone().ok_or("attention is stopped")?;
        self.sequence = next(self.sequence)?;
        let request = CaptureRequest {
            generation: self.generation,
            sequence: self.sequence,
            target,
            requested_at_ms: now_ms,
        };
        self.pending = Some(request.clone());
        Ok(request)
    }

    pub fn accept(&mut self, packet: ContextPacket, now_ms: u64) -> Result<ContextPacket> {
        if self.target.is_none() || now_ms >= self.expires_at_ms {
            return Err("attention is no longer active".into());
        }
        if self.pending.as_ref() != Some(&packet.request)
            || packet.observed_target != packet.request.target
            || packet.observed_at_ms < packet.request.requested_at_ms
            || packet.observed_at_ms > now_ms
            || now_ms.saturating_sub(packet.observed_at_ms) > 5000
        {
            return Err("context is stale, mismatched, or from another window".into());
        }
        if let Some(selection) = &packet.selected_text {
            if selection.len() > MAX_TEXT_BYTES {
                return Err("selection exceeds context budget".into());
            }
        }
        if let Some(image) = &packet.image_ref {
            text(image)?;
        }
        self.pending = None;
        Ok(packet)
    }

    pub fn stop(&mut self) {
        self.target = None;
        self.pending = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Normal,
    Restricted,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    pub id: String,
    /// Host-computed digest/immutable identity of the actual bytes, not an LLM assertion.
    pub snapshot: String,
    pub policy_generation: u64,
    pub sensitivity: Sensitivity,
}

impl SourceRef {
    pub fn validate(&self) -> Result<()> {
        text(&self.id)?;
        text(&self.snapshot)
    }
}

/// No restricted override in this contract. `allow_cloud` is trusted host
/// configuration, not a tool argument. Call immediately before network egress,
/// using freshly re-read references for every contributing source (also for
/// summaries). A local-readable file is not automatically cloud-shareable.
pub fn validate_cloud_release(
    allow_cloud: bool,
    requested: &[SourceRef],
    current: &[SourceRef],
) -> Result<()> {
    if !allow_cloud || requested.is_empty() || requested.len() > MAX_ITEMS {
        return Err("cloud release not authorized or source budget exceeded".into());
    }
    if current.len() != requested.len() {
        return Err("source attestation is incomplete".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for source in requested {
        source.validate()?;
        if !seen.insert(&source.id)
            || source.sensitivity != Sensitivity::Normal
            || current.iter().filter(|s| s.id == source.id).count() != 1
            || !current.contains(source)
        {
            return Err("source changed, is duplicated, or cannot leave this device".into());
        }
    }
    Ok(())
}
