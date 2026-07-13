use super::{Nudge, NudgeDraft, COPILOT_CONTRACT_VERSION};
use chrono::{DateTime, Utc};

/// Applies deterministic lifetime, evidence, and supersession rules to
/// provider output.
#[derive(Debug)]
pub struct NudgePolicy {
    ttl_ms: u64,
    next_id: u64,
    active: Option<Nudge>,
}

impl NudgePolicy {
    pub fn new(ttl_ms: u64) -> Self {
        Self {
            ttl_ms: ttl_ms.max(1),
            next_id: 1,
            active: None,
        }
    }

    pub fn active_at(&mut self, now: DateTime<Utc>) -> Option<&Nudge> {
        if self
            .active
            .as_ref()
            .is_some_and(|nudge| nudge.is_expired_at(now))
        {
            self.active = None;
        }
        self.active.as_ref()
    }

    pub fn accept(
        &mut self,
        mut draft: NudgeDraft,
        evidence_revision: u64,
        now: DateTime<Utc>,
    ) -> Option<Nudge> {
        draft.text = truncate_chars(draft.text.trim(), 240);
        draft.source_chip = truncate_chars(draft.source_chip.trim(), 64);
        if draft.text.is_empty() || draft.source_chip.is_empty() {
            return None;
        }

        let supersedes = self.active_at(now).map(|nudge| nudge.id.clone());
        let id = format!("nudge-{evidence_revision}-{}", self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        let nudge = Nudge {
            v: COPILOT_CONTRACT_VERSION,
            id,
            kind: draft.kind,
            text: draft.text,
            source_chip: draft.source_chip,
            evidence_revision,
            created_ts: now,
            ttl_ms: self.ttl_ms,
            supersedes,
        };
        self.active = Some(nudge.clone());
        Some(nudge)
    }

    pub fn clear(&mut self) {
        self.active = None;
    }
}

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        value.to_string()
    } else {
        value.chars().take(limit).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copilot::NudgeKind;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 13, 20, 0, 0)
            .single()
            .unwrap()
    }

    fn draft(text: &str) -> NudgeDraft {
        NudgeDraft {
            kind: NudgeKind::Ask,
            text: text.into(),
            source_chip: "rollout".into(),
        }
    }

    #[test]
    fn nudge_expires_at_ttl_boundary() {
        let mut policy = NudgePolicy::new(12_000);
        let nudge = policy.accept(draft("Ask for a date"), 42, now()).unwrap();
        assert!(!nudge.is_expired_at(now() + chrono::Duration::milliseconds(11_999)));
        assert!(nudge.is_expired_at(now() + chrono::Duration::milliseconds(12_000)));
        assert!(policy
            .active_at(now() + chrono::Duration::milliseconds(12_000))
            .is_none());
    }

    #[test]
    fn newer_nudge_supersedes_active_nudge_only() {
        let mut policy = NudgePolicy::new(12_000);
        let first = policy.accept(draft("Ask for a date"), 42, now()).unwrap();
        let second = policy
            .accept(
                draft("Clarify the owner"),
                43,
                now() + chrono::Duration::seconds(1),
            )
            .unwrap();
        assert_eq!(second.supersedes, Some(first.id));

        let third = policy
            .accept(
                draft("Ask what changed"),
                44,
                now() + chrono::Duration::seconds(20),
            )
            .unwrap();
        assert_eq!(third.supersedes, None);
    }
}
