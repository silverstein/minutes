use super::{
    CopilotRequest, MeetingMode, MeetingModePolicy, Nudge, NudgeDraft, COPILOT_CONTRACT_VERSION,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopilotFeedback {
    Dismissed,
    Helpful,
    NotHelpful,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicySnapshot {
    pub mode: MeetingMode,
    pub base_minimum_interval_ms: u64,
    pub effective_minimum_interval_ms: u64,
    pub base_minimum_confidence: u8,
    pub effective_minimum_confidence: u8,
    pub helpful: u32,
    pub not_helpful: u32,
    pub dismissed: u32,
    pub accepted: u32,
    pub filtered_by_mode: u32,
    pub filtered_by_threshold: u32,
    pub filtered_by_cadence: u32,
}

impl Default for PolicySnapshot {
    fn default() -> Self {
        Self {
            mode: MeetingMode::Generic,
            base_minimum_interval_ms: 0,
            effective_minimum_interval_ms: 0,
            base_minimum_confidence: 0,
            effective_minimum_confidence: 0,
            helpful: 0,
            not_helpful: 0,
            dismissed: 0,
            accepted: 0,
            filtered_by_mode: 0,
            filtered_by_threshold: 0,
            filtered_by_cadence: 0,
        }
    }
}

/// Applies deterministic lifetime, evidence, and supersession rules to
/// provider output.
#[derive(Debug)]
pub struct NudgePolicy {
    ttl_ms: u64,
    next_id: u64,
    active: Option<Nudge>,
    mode_policy: MeetingModePolicy,
    snapshot: PolicySnapshot,
    last_nudge_at: Option<DateTime<Utc>>,
    feedback_nudges: HashSet<String>,
    issued_nudges: HashSet<String>,
    issued_order: VecDeque<String>,
}

impl NudgePolicy {
    pub fn new(ttl_ms: u64) -> Self {
        Self::for_mode(ttl_ms, MeetingMode::Generic)
    }

    pub fn for_mode(ttl_ms: u64, mode: MeetingMode) -> Self {
        let mode_policy = mode.policy();
        Self {
            ttl_ms: ttl_ms.max(1),
            next_id: 1,
            active: None,
            mode_policy,
            snapshot: PolicySnapshot {
                mode,
                base_minimum_interval_ms: mode_policy.minimum_interval_ms,
                effective_minimum_interval_ms: mode_policy.minimum_interval_ms,
                base_minimum_confidence: mode_policy.minimum_confidence,
                effective_minimum_confidence: mode_policy.minimum_confidence,
                ..PolicySnapshot::default()
            },
            last_nudge_at: None,
            feedback_nudges: HashSet::new(),
            issued_nudges: HashSet::new(),
            issued_order: VecDeque::new(),
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
        draft.confidence = draft.confidence.min(100);
        if !self.mode_policy.allows(draft.opportunity) {
            self.snapshot.filtered_by_mode = self.snapshot.filtered_by_mode.saturating_add(1);
            return None;
        }
        if draft.confidence < self.snapshot.effective_minimum_confidence {
            self.snapshot.filtered_by_threshold =
                self.snapshot.filtered_by_threshold.saturating_add(1);
            return None;
        }
        if self.last_nudge_at.is_some_and(|last| {
            now.signed_duration_since(last).num_milliseconds()
                < self
                    .snapshot
                    .effective_minimum_interval_ms
                    .min(i64::MAX as u64) as i64
        }) {
            self.snapshot.filtered_by_cadence = self.snapshot.filtered_by_cadence.saturating_add(1);
            return None;
        }

        let supersedes = self.active_at(now).map(|nudge| nudge.id.clone());
        let id = format!("nudge-{}-{}", request.evidence_revision, self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        let grounded_partial_identity = request.grounded_partial_identity();
        let nudge = Nudge {
            v: COPILOT_CONTRACT_VERSION,
            id,
            kind: draft.kind,
            text: draft.text,
            source_chip: draft.source_chip,
            opportunity: draft.opportunity,
            confidence: draft.confidence,
            session_epoch: request.session_epoch,
            evidence_revision: request.evidence_revision,
            evidence_utterance_sequence: request.evidence_utterance_sequence,
            evidence_utterance_revision: request.evidence_utterance_revision,
            grounded_partial_utterance_sequence: grounded_partial_identity
                .map(|(sequence, _)| sequence),
            grounded_partial_utterance_revision: grounded_partial_identity
                .map(|(_, revision)| revision),
            update_kind: request.update_kind,
            created_ts: now,
            ttl_ms: self.ttl_ms,
            supersedes,
        };
        self.active = Some(nudge.clone());
        self.issued_nudges.insert(nudge.id.clone());
        self.issued_order.push_back(nudge.id.clone());
        if self.issued_order.len() > 128 {
            if let Some(expired) = self.issued_order.pop_front() {
                self.issued_nudges.remove(&expired);
                self.feedback_nudges.remove(&expired);
            }
        }
        self.last_nudge_at = Some(now);
        self.snapshot.accepted = self.snapshot.accepted.saturating_add(1);
        Some(nudge)
    }

    /// Apply bounded session-only adaptation. Mode eligibility and tone are
    /// immutable, so feedback cannot weaken safety or history rules.
    pub fn record_feedback(&mut self, nudge_id: &str, feedback: CopilotFeedback) -> bool {
        if nudge_id.trim().is_empty()
            || !self.issued_nudges.contains(nudge_id)
            || !self.feedback_nudges.insert(nudge_id.to_string())
        {
            return false;
        }
        match feedback {
            CopilotFeedback::Helpful => {
                self.snapshot.helpful = self.snapshot.helpful.saturating_add(1)
            }
            CopilotFeedback::NotHelpful => {
                self.snapshot.not_helpful = self.snapshot.not_helpful.saturating_add(1)
            }
            CopilotFeedback::Dismissed => {
                self.snapshot.dismissed = self.snapshot.dismissed.saturating_add(1);
                if self
                    .active
                    .as_ref()
                    .is_some_and(|nudge| nudge.id == nudge_id)
                {
                    self.active = None;
                }
            }
        }
        self.recompute_effective_policy();
        true
    }

    pub fn snapshot(&self) -> PolicySnapshot {
        self.snapshot.clone()
    }

    fn recompute_effective_policy(&mut self) {
        let quieter_ms = u64::from(self.snapshot.dismissed)
            .saturating_mul(4_000)
            .saturating_add(u64::from(self.snapshot.not_helpful).saturating_mul(8_000));
        let helpful_ms = u64::from(self.snapshot.helpful).saturating_mul(1_000);
        self.snapshot.effective_minimum_interval_ms = self
            .snapshot
            .base_minimum_interval_ms
            .saturating_add(quieter_ms)
            .saturating_sub(helpful_ms)
            .min(60_000);

        let stricter = self
            .snapshot
            .dismissed
            .saturating_mul(5)
            .saturating_add(self.snapshot.not_helpful.saturating_mul(10));
        let looser = self.snapshot.helpful.saturating_mul(3);
        self.snapshot.effective_minimum_confidence =
            u32::from(self.snapshot.base_minimum_confidence)
                .saturating_add(stricter)
                .saturating_sub(looser)
                .min(95) as u8;
    }

    pub fn clear(&mut self) {
        self.active = None;
    }

    pub fn reset_session(&mut self) {
        self.active = None;
        self.last_nudge_at = None;
        self.feedback_nudges.clear();
        self.issued_nudges.clear();
        self.issued_order.clear();
        self.snapshot = PolicySnapshot {
            mode: self.mode_policy.mode,
            base_minimum_interval_ms: self.mode_policy.minimum_interval_ms,
            effective_minimum_interval_ms: self.mode_policy.minimum_interval_ms,
            base_minimum_confidence: self.mode_policy.minimum_confidence,
            effective_minimum_confidence: self.mode_policy.minimum_confidence,
            ..PolicySnapshot::default()
        };
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
            opportunity: super::super::OpportunityKind::General,
            confidence: 100,
        }
    }

    fn request(revision: u64) -> CopilotRequest {
        CopilotRequest {
            goal: "close next steps".into(),
            mode: MeetingMode::Generic,
            session_epoch: 1,
            evidence_revision: revision,
            evidence_utterance_sequence: 1,
            evidence_utterance_revision: revision,
            update_kind: TranscriptUpdateKind::Partial,
            utterances: Vec::new(),
            battle_card: BattleCard::empty(),
            strategy_state: super::super::StrategyState::empty(),
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

    #[test]
    fn negative_feedback_raises_only_session_cadence_and_threshold() {
        let mut policy = NudgePolicy::for_mode(12_000, MeetingMode::Sales);
        let nudge = policy
            .accept(draft("Ask for a date"), &request(1), now())
            .unwrap();
        assert!(policy.record_feedback(&nudge.id, CopilotFeedback::Dismissed));
        let snapshot = policy.snapshot();
        assert_eq!(snapshot.mode, MeetingMode::Sales);
        assert!(snapshot.effective_minimum_interval_ms > snapshot.base_minimum_interval_ms);
        assert!(snapshot.effective_minimum_confidence > snapshot.base_minimum_confidence);
        assert_eq!(policy.mode_policy.tone, MeetingMode::Sales.policy().tone);
    }
}
