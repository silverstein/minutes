//! Exact-payload approval. A model-visible proposal ID is not an approval token.

use serde::{Deserialize, Serialize};

use super::{next, text, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub operation: String,
    pub account: String,
    pub recipient_or_target: String,
    /// Full canonical payload, including body, attachments and other effects.
    pub payload: String,
}

impl Action {
    pub fn validate(&self) -> Result<()> {
        text(&self.operation)?;
        text(&self.account)?;
        text(&self.recipient_or_target)?;
        text(&self.payload)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Proposal {
    pub id: u64,
    pub action: Action,
    pub expires_at_ms: u64,
    pub policy_generation: u64,
}

#[derive(Debug)]
struct Pending {
    proposal: Proposal,
    created_at_ms: u64,
    approved: bool,
}

/// Session-local, bounded to one review. Never serialize or expose its approval
/// methods as model tools. OS permission is a separate prerequisite at execution.
#[derive(Debug, Default)]
pub struct ApprovalGate {
    sequence: u64,
    pending: Option<Pending>,
}

impl ApprovalGate {
    pub fn propose(
        &mut self,
        action: Action,
        now_ms: u64,
        ttl_ms: u64,
        policy_generation: u64,
    ) -> Result<Proposal> {
        action.validate()?;
        if ttl_ms == 0 || ttl_ms > 5 * 60 * 1000 {
            return Err("approval expires within five minutes".into());
        }
        let expires_at_ms = now_ms.checked_add(ttl_ms).ok_or("clock overflow")?;
        self.sequence = next(self.sequence)?;
        let proposal = Proposal {
            id: self.sequence,
            action,
            expires_at_ms,
            policy_generation,
        };
        self.pending = Some(Pending {
            proposal: proposal.clone(),
            created_at_ms: now_ms,
            approved: false,
        });
        Ok(proposal)
    }

    /// Trusted UI/CLI gesture only. Supply the exact review the human saw.
    /// An input-transcript event, another utterance, or model tool call must
    /// never call this method. Corrections require a new proposal and review.
    pub fn approve_from_host(
        &mut self,
        reviewed: &Proposal,
        now_ms: u64,
        policy_generation: u64,
    ) -> Result<()> {
        let pending = self.pending.as_mut().ok_or("no pending review")?;
        if pending.proposal != *reviewed {
            return Err("review no longer matches the pending action".into());
        }
        check_fresh(pending, now_ms, policy_generation)?;
        pending.approved = true;
        Ok(())
    }

    pub fn reject_from_host(&mut self) {
        self.pending = None;
    }

    /// Moves the stored action out exactly once. Execute THESE bytes, not
    /// arguments resubmitted by the model. Failure/unknown delivery requires a
    /// new review; reconnect must not replay an outward operation.
    pub fn take_approved(
        &mut self,
        id: u64,
        now_ms: u64,
        policy_generation: u64,
    ) -> Result<Action> {
        let pending = self.pending.as_ref().ok_or("no pending review")?;
        check_fresh(pending, now_ms, policy_generation)?;
        if pending.proposal.id != id || !pending.approved {
            return Err("action has not been approved by the host".into());
        }
        self.pending
            .take()
            .map(|p| p.proposal.action)
            .ok_or_else(|| "no pending review".into())
    }
}

fn check_fresh(pending: &Pending, now_ms: u64, policy_generation: u64) -> Result<()> {
    if now_ms < pending.created_at_ms
        || now_ms >= pending.proposal.expires_at_ms
        || policy_generation != pending.proposal.policy_generation
    {
        return Err("approval expired or policy changed".into());
    }
    Ok(())
}
