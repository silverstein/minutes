use super::{CopilotRequest, Nudge, NudgeDraft, COPILOT_CONTRACT_VERSION};
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
        request: &CopilotRequest,
        now: DateTime<Utc>,
    ) -> Option<Nudge> {
        draft.text = truncate_chars(draft.text.trim(), 240);
        draft.source_chip = truncate_chars(draft.source_chip.trim(), 64);
        if draft.text.is_empty() || draft.source_chip.is_empty() {
            return None;
        }

        let supersedes = self.active_at(now).map(|nudge| nudge.id.clone());
        let id = format!("nudge-{}-{}", request.evidence_revision, self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        let nudge = Nudge {
            v: COPILOT_CONTRACT_VERSION,
            id,
            kind: draft.kind,
            text: draft.text,
            source_chip: draft.source_chip,
            session_epoch: request.session_epoch,
            evidence_revision: request.evidence_revision,
            evidence_utterance_sequence: request.evidence_utterance_sequence,
            evidence_utterance_revision: request.evidence_utterance_revision,
            update_kind: request.update_kind,
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
    use crate::copilot::{BattleCard, NudgeKind, TranscriptUpdateKind};
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

    fn request(revision: u64) -> CopilotRequest {
        CopilotRequest {
            goal: "close next steps".into(),
            session_epoch: 1,
            evidence_revision: revision,
            evidence_utterance_sequence: 1,
            evidence_utterance_revision: revision,
            update_kind: TranscriptUpdateKind::Partial,
            utterances: Vec::new(),
            battle_card: BattleCard::empty(),
        }
    }

    #[test]
    fn nudge_expires_at_ttl_boundary() {
        let mut policy = NudgePolicy::new(12_000);
        let nudge = policy
            .accept(draft("Ask for a date"), &request(42), now())
            .unwrap();
        assert!(!nudge.is_expired_at(now() + chrono::Duration::milliseconds(11_999)));
        assert!(nudge.is_expired_at(now() + chrono::Duration::milliseconds(12_000)));
        assert!(policy
            .active_at(now() + chrono::Duration::milliseconds(12_000))
            .is_none());
    }

    #[test]
    fn newer_nudge_supersedes_active_nudge_only() {
        let mut policy = NudgePolicy::new(12_000);
        let first = policy
            .accept(draft("Ask for a date"), &request(42), now())
            .unwrap();
        let second = policy
            .accept(
                draft("Clarify the owner"),
                &request(43),
                now() + chrono::Duration::seconds(1),
            )
            .unwrap();
        assert_eq!(second.supersedes, Some(first.id));

        let third = policy
            .accept(
                draft("Ask what changed"),
                &request(44),
                now() + chrono::Duration::seconds(20),
            )
            .unwrap();
        assert_eq!(third.supersedes, None);
    }
}
