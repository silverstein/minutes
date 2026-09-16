//! Portable work continuity. No capture, network, shell, UI, or implicit actions.
//!
//! A checkpoint is data, never authority. Only a trusted host may call the
//! approval/disclosure methods; do not expose them as model-callable tools.
//! The existing LiveAssistanceSession remains the live-capture reducer. This
//! aggregate owns the work that can outlive a live/model connection.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

const MAX_ITEMS: usize = 128;
const MAX_TEXT: usize = 16_384;
pub const MAX_CHECKPOINT_BYTES: usize = 1_048_576;
const CHECKPOINT_PREFIX: &str = "# Minutes work checkpoint\n\n<!-- minutes:work:v1 -->\n```json\n";
const CHECKPOINT_SUFFIX: &str = "\n```\n";

#[derive(Debug, thiserror::Error)]
pub enum WorkError {
    #[error("invalid work data: {0}")]
    Invalid(&'static str),
    #[error("record not found")]
    NotFound,
    #[error("stale context, policy, or task generation")]
    Stale,
    #[error("operation is not allowed in this state")]
    InvalidTransition,
    #[error("host approval required, expired, revoked, or already consumed")]
    ApprovalRequired,
    #[error("explicit, source-bound cloud disclosure approval required")]
    DisclosureDenied,
    #[error("checkpoint encoding: {0}")]
    Encoding(#[from] serde_json::Error),
}

fn text(value: &str) -> Result<(), WorkError> {
    if value.trim().is_empty() || value.len() > MAX_TEXT || value.contains('\0') {
        return Err(WorkError::Invalid("empty, NUL-containing, or oversized text"));
    }
    Ok(())
}

fn identifier(value: &str) -> Result<(), WorkError> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
    {
        return Err(WorkError::Invalid("invalid identifier"));
    }
    Ok(())
}

fn next(value: u64) -> Result<u64, WorkError> {
    value.checked_add(1).ok_or(WorkError::Invalid("generation exhausted"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Normal,
    Restricted,
    Unknown,
}

/// Metadata is not proof of a successful read. The host must attest a source
/// from live bytes/policy before release or actuation; imported refs are hints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRef {
    pub id: String,
    pub locator: String,
    pub version: String,
    pub observed_at_ms: u64,
    pub sensitivity: Sensitivity,
}

impl SourceRef {
    fn validate(&self) -> Result<(), WorkError> {
        identifier(&self.id)?;
        text(&self.locator)?;
        text(&self.version)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Focus {
    pub source: SourceRef,
    /// A stable document range/selection reference, not a mouse coordinate.
    pub selection: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Attribution {
    SourceRecord,
    UserInterpretation,
    ModelInference,
    UserConfirmedDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryEntry {
    pub attribution: Attribution,
    pub text: String,
    pub source_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Running,
    AwaitingInput,
    CancelRequested,
    Cancelled,
    Completed,
    CompletedAfterCancelRequest,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRecord {
    pub id: String,
    pub instruction: String,
    pub generation: u64,
    pub state: TaskState,
    pub artifact: Option<SourceRef>,
    /// Speech is a briefing; it must not replace or truncate the work artifact.
    pub spoken_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkCheckpoint {
    pub schema_version: u32,
    pub id: String,
    pub goal: String,
    pub revision: u64,
    pub focus: Option<Focus>,
    pub sources: Vec<SourceRef>,
    pub memory: Vec<MemoryEntry>,
    pub open_questions: Vec<String>,
    pub next_step: Option<String>,
    pub tasks: Vec<TaskRecord>,
}

impl WorkCheckpoint {
    pub fn validate(&self) -> Result<(), WorkError> {
        if self.schema_version != 1 {
            return Err(WorkError::Invalid("unsupported checkpoint version"));
        }
        identifier(&self.id)?;
        text(&self.goal)?;
        if [self.sources.len(), self.memory.len(), self.open_questions.len(), self.tasks.len()]
            .iter().any(|n| *n > MAX_ITEMS)
        {
            return Err(WorkError::Invalid("too many records"));
        }
        let mut sources = BTreeMap::new();
        for source in &self.sources {
            source.validate()?;
            if sources.insert(source.id.as_str(), source).is_some() {
                return Err(WorkError::Invalid("duplicate source id"));
            }
        }
        if let Some(focus) = &self.focus {
            text(&focus.selection)?;
            if sources.get(focus.source.id.as_str()).copied() != Some(&focus.source) {
                return Err(WorkError::Invalid("focus must match an exact source version"));
            }
        }
        for entry in &self.memory {
            text(&entry.text)?;
            if entry.source_ids.len() > MAX_ITEMS
                || entry.source_ids.iter().any(|id| !sources.contains_key(id.as_str()))
            {
                return Err(WorkError::Invalid("memory contains missing source references"));
            }
            if entry.attribution == Attribution::SourceRecord && entry.source_ids.is_empty() {
                return Err(WorkError::Invalid("source record requires provenance"));
            }
        }
        for question in &self.open_questions {
            text(question)?;
        }
        if let Some(step) = &self.next_step {
            text(step)?;
        }
        let mut tasks = BTreeMap::new();
        for task in &self.tasks {
            identifier(&task.id)?;
            text(&task.instruction)?;
            if tasks.insert(&task.id, ()).is_some() {
                return Err(WorkError::Invalid("duplicate task id"));
            }
            if let Some(artifact) = &task.artifact {
                artifact.validate()?;
            }
            if let Some(summary) = &task.spoken_summary {
                text(summary)?;
            }
            let completed = matches!(task.state, TaskState::Completed | TaskState::CompletedAfterCancelRequest);
            if completed != task.artifact.is_some() || (!completed && task.spoken_summary.is_some()) {
                return Err(WorkError::Invalid("task outcome does not match state"));
            }
        }
        if serde_json::to_vec(self)?.len() > MAX_CHECKPOINT_BYTES / 2 {
            return Err(WorkError::Invalid("checkpoint exceeds aggregate byte budget"));
        }
        Ok(())
    }

    /// No filesystem side effects. Persist through the host's private,
    /// capability-bound storage, not an unrestricted model-supplied path.
    pub fn to_markdown(&self) -> Result<String, WorkError> {
        self.validate()?;
        let body = serde_json::to_string_pretty(self)?;
        let encoded = format!("{CHECKPOINT_PREFIX}{body}{CHECKPOINT_SUFFIX}");
        if encoded.len() > MAX_CHECKPOINT_BYTES {
            return Err(WorkError::Invalid("encoded checkpoint exceeds byte budget"));
        }
        Ok(encoded)
    }

    pub fn from_markdown(input: &str) -> Result<Self, WorkError> {
        if input.len() > MAX_CHECKPOINT_BYTES {
            return Err(WorkError::Invalid("checkpoint exceeds byte budget"));
        }
        let json = input.strip_prefix(CHECKPOINT_PREFIX)
            .and_then(|s| s.strip_suffix(CHECKPOINT_SUFFIX))
            .ok_or(WorkError::Invalid("not a work checkpoint"))?;
        let checkpoint: Self = serde_json::from_str(json)?;
        checkpoint.validate()?;
        Ok(checkpoint)
    }
}

/// Deliberately not serializable or constructible from model output.
#[derive(Debug)]
pub struct RunLease {
    instance: Arc<()>,
    task_id: String,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedAction {
    pub verb: String,
    pub target: String,
    pub payload: serde_json::Value,
}

impl ProposedAction {
    fn validate(&self) -> Result<(), WorkError> {
        identifier(&self.verb)?;
        text(&self.target)?;
        if !self.payload.is_object() || serde_json::to_vec(&self.payload)?.len() > MAX_TEXT {
            return Err(WorkError::Invalid("action payload must be a bounded object"));
        }
        Ok(())
    }
}

#[derive(Debug)]
struct PendingAction {
    action: ProposedAction,
    revision: u64,
    expires_ms: u64,
    approved: bool,
}

/// In-process host capability; not an opaque string handed to the model.
#[derive(Debug)]
pub struct ApprovalPermit {
    instance: Arc<()>,
    proposal: u64,
    revision: u64,
}

#[derive(Debug)]
pub struct AuthorizedAction(ProposedAction);

impl AuthorizedAction {
    pub fn action(&self) -> &ProposedAction {
        &self.0
    }
}

/// A separate permission from local read access. Never persisted in memory.
#[derive(Debug)]
pub struct CloudPermit {
    instance: Arc<()>,
    destination: String,
    revision: u64,
    policy_generation: u64,
    expires_ms: u64,
    sources: Vec<SourceRef>,
}

#[derive(Debug)]
pub struct WorkSession {
    data: WorkCheckpoint,
    instance: Arc<()>,
    policy_generation: u64,
    next_proposal: u64,
    pending: BTreeMap<u64, PendingAction>,
}

impl WorkSession {
    pub fn new(id: String, goal: String) -> Result<Self, WorkError> {
        let data = WorkCheckpoint {
            schema_version: 1, id, goal, revision: 0, focus: None, sources: vec![],
            memory: vec![], open_questions: vec![], next_step: None, tasks: vec![],
        };
        data.validate()?;
        Ok(Self { data, instance: Arc::new(()), policy_generation: 0, next_proposal: 0, pending: BTreeMap::new() })
    }

    /// Restores context, not running processes, grants, approval, or capability.
    /// Even queued work requires a fresh, explicit host restart after restore.
    pub fn resume(mut data: WorkCheckpoint) -> Result<Self, WorkError> {
        data.validate()?;
        data.revision = next(data.revision)?;
        for task in &mut data.tasks {
            if matches!(task.state, TaskState::Queued | TaskState::Running | TaskState::AwaitingInput | TaskState::CancelRequested) {
                task.state = TaskState::Interrupted;
                task.generation = next(task.generation)?;
            }
        }
        Ok(Self { data, instance: Arc::new(()), policy_generation: 0, next_proposal: 0, pending: BTreeMap::new() })
    }

    pub fn checkpoint(&self) -> WorkCheckpoint {
        self.data.clone()
    }

    pub fn revision(&self) -> u64 {
        self.data.revision
    }

    fn update(&mut self, mut candidate: WorkCheckpoint) -> Result<(), WorkError> {
        candidate.revision = next(self.data.revision)?;
        candidate.validate()?;
        self.data = candidate;
        self.pending.clear();
        Ok(())
    }

    /// Host input: capture the selection before an overlay steals focus.
    pub fn set_focus(&mut self, focus: Focus) -> Result<(), WorkError> {
        focus.source.validate()?;
        let mut data = self.data.clone();
        if let Some(existing) = data.sources.iter().find(|s| s.id == focus.source.id) {
            // A source version is immutable; use a new id for a new snapshot.
            if existing != &focus.source {
                return Err(WorkError::Stale);
            }
        } else {
            data.sources.push(focus.source.clone());
        }
        data.focus = Some(focus);
        self.update(data)
    }

    pub fn revise_goal(&mut self, goal: String) -> Result<(), WorkError> {
        let mut data = self.data.clone();
        data.goal = goal;
        self.update(data)
    }

    /// Model-written entries cannot declare themselves human-confirmed.
    pub fn add_model_note(&mut self, note: String, source_ids: Vec<String>) -> Result<(), WorkError> {
        self.add_entry(MemoryEntry { attribution: Attribution::ModelInference, text: note, source_ids })
    }

    pub fn add_host_note(&mut self, note: String, source_ids: Vec<String>, decision: bool) -> Result<(), WorkError> {
        self.add_entry(MemoryEntry {
            attribution: if decision { Attribution::UserConfirmedDecision } else { Attribution::UserInterpretation },
            text: note, source_ids,
        })
    }

    fn add_entry(&mut self, entry: MemoryEntry) -> Result<(), WorkError> {
        let mut data = self.data.clone();
        data.memory.push(entry);
        self.update(data)
    }

    pub fn park(&mut self, questions: Vec<String>, next_step: Option<String>) -> Result<WorkCheckpoint, WorkError> {
        let mut data = self.data.clone();
        data.open_questions = questions;
        data.next_step = next_step;
        // Parking revokes pending actions but does not claim to kill workers.
        self.update(data)?;
        Ok(self.checkpoint())
    }

    pub fn enqueue_task(&mut self, id: String, instruction: String) -> Result<(), WorkError> {
        let mut data = self.data.clone();
        data.tasks.push(TaskRecord { id, instruction, generation: 0, state: TaskState::Queued, artifact: None, spoken_summary: None });
        self.update(data)
    }

    pub fn task(&self, id: &str) -> Result<&TaskRecord, WorkError> {
        self.data.tasks.iter().find(|t| t.id == id).ok_or(WorkError::NotFound)
    }

    pub fn start_task(&mut self, id: &str) -> Result<RunLease, WorkError> {
        let mut data = self.data.clone();
        let task = data.tasks.iter_mut().find(|t| t.id == id).ok_or(WorkError::NotFound)?;
        if task.state != TaskState::Queued {
            return Err(WorkError::InvalidTransition);
        }
        task.state = TaskState::Running;
        let generation = task.generation;
        self.update(data)?;
        Ok(RunLease { instance: Arc::clone(&self.instance), task_id: id.into(), generation })
    }

    fn check_lease(&self, lease: &RunLease) -> Result<(), WorkError> {
        if !Arc::ptr_eq(&self.instance, &lease.instance) || self.task(&lease.task_id)?.generation != lease.generation {
            return Err(WorkError::Stale);
        }
        Ok(())
    }

    /// Return state distinguishes a request from confirmed worker termination.
    pub fn request_cancel(&mut self, id: &str) -> Result<TaskState, WorkError> {
        let mut data = self.data.clone();
        let task = data.tasks.iter_mut().find(|t| t.id == id).ok_or(WorkError::NotFound)?;
        task.state = match task.state {
            TaskState::Queued => TaskState::Cancelled,
            TaskState::Running | TaskState::AwaitingInput => TaskState::CancelRequested,
            TaskState::CancelRequested | TaskState::Cancelled => task.state,
            _ => return Err(WorkError::InvalidTransition),
        };
        let state = task.state;
        self.update(data)?;
        Ok(state)
    }

    pub fn acknowledge_cancel(&mut self, lease: RunLease) -> Result<(), WorkError> {
        self.check_lease(&lease)?;
        let mut data = self.data.clone();
        let task = data.tasks.iter_mut().find(|t| t.id == lease.task_id).ok_or(WorkError::NotFound)?;
        if task.state != TaskState::CancelRequested {
            return Err(WorkError::InvalidTransition);
        }
        task.state = TaskState::Cancelled;
        self.update(data)
    }

    pub fn fail_task(&mut self, lease: RunLease) -> Result<(), WorkError> {
        self.check_lease(&lease)?;
        let mut data = self.data.clone();
        let task = data.tasks.iter_mut().find(|t| t.id == lease.task_id).ok_or(WorkError::NotFound)?;
        if !matches!(task.state, TaskState::Running | TaskState::AwaitingInput | TaskState::CancelRequested) {
            return Err(WorkError::InvalidTransition);
        }
        task.state = TaskState::Failed;
        self.update(data)
    }

    pub fn set_awaiting_input(&mut self, lease: &RunLease, waiting: bool) -> Result<(), WorkError> {
        self.check_lease(lease)?;
        let mut data = self.data.clone();
        let task = data.tasks.iter_mut().find(|t| t.id == lease.task_id).ok_or(WorkError::NotFound)?;
        if !matches!(task.state, TaskState::Running | TaskState::AwaitingInput) {
            return Err(WorkError::InvalidTransition);
        }
        task.state = if waiting { TaskState::AwaitingInput } else { TaskState::Running };
        self.update(data)
    }

    /// Requeue only after old work has stopped. A caller must not start a new
    /// worker while an earlier worker is merely cancel-requested.
    pub fn revise_task(&mut self, id: &str, instruction: String) -> Result<(), WorkError> {
        text(&instruction)?;
        let mut data = self.data.clone();
        let task = data.tasks.iter_mut().find(|t| t.id == id).ok_or(WorkError::NotFound)?;
        if !matches!(task.state, TaskState::Queued | TaskState::Cancelled | TaskState::Failed | TaskState::Interrupted) {
            return Err(WorkError::InvalidTransition);
        }
        task.instruction = instruction;
        task.generation = next(task.generation)?;
        task.state = TaskState::Queued;
        task.artifact = None;
        task.spoken_summary = None;
        self.update(data)
    }

    pub fn complete_task(&mut self, lease: RunLease, artifact: SourceRef, summary: String) -> Result<(), WorkError> {
        self.check_lease(&lease)?;
        artifact.validate()?;
        text(&summary)?;
        let mut data = self.data.clone();
        let task = data.tasks.iter_mut().find(|t| t.id == lease.task_id).ok_or(WorkError::NotFound)?;
        task.state = match task.state {
            TaskState::Running | TaskState::AwaitingInput => TaskState::Completed,
            TaskState::CancelRequested => TaskState::CompletedAfterCancelRequest,
            _ => return Err(WorkError::InvalidTransition),
        };
        task.artifact = Some(artifact);
        task.spoken_summary = Some(summary);
        self.update(data)
    }

    pub fn propose_action(&mut self, action: ProposedAction, now_ms: u64, ttl_ms: u64) -> Result<u64, WorkError> {
        action.validate()?;
        if self.pending.len() >= MAX_ITEMS || ttl_ms == 0 || ttl_ms > 60_000 {
            return Err(WorkError::Invalid("proposal budget or lifetime exceeded"));
        }
        let expires_ms = now_ms.checked_add(ttl_ms).ok_or(WorkError::Invalid("clock overflow"))?;
        let id = next(self.next_proposal)?;
        self.next_proposal = id;
        self.pending.insert(id, PendingAction { action, revision: self.data.revision, expires_ms, approved: false });
        Ok(id)
    }

    /// The trusted UI must display ALL fields and pass back the exact preview.
    /// An ASR turn, model 'yes', or model-provided token must not call this.
    pub fn approve_from_host(&mut self, id: u64, displayed: &ProposedAction, revision: u64, now_ms: u64) -> Result<ApprovalPermit, WorkError> {
        let pending = self.pending.get_mut(&id).ok_or(WorkError::ApprovalRequired)?;
        if pending.approved || pending.action != *displayed || pending.revision != revision
            || self.data.revision != revision || now_ms >= pending.expires_ms
        {
            return Err(WorkError::ApprovalRequired);
        }
        pending.approved = true;
        Ok(ApprovalPermit { instance: Arc::clone(&self.instance), proposal: id, revision })
    }

    /// Denials, corrections, and 'wait' revoke; they never count as approval.
    pub fn reject_from_host(&mut self, id: u64) -> Result<(), WorkError> {
        self.pending.remove(&id).map(|_| ()).ok_or(WorkError::NotFound)
    }

    pub fn consume_approval(&mut self, permit: ApprovalPermit, now_ms: u64) -> Result<AuthorizedAction, WorkError> {
        if !Arc::ptr_eq(&self.instance, &permit.instance) || self.data.revision != permit.revision {
            return Err(WorkError::ApprovalRequired);
        }
        let pending = self.pending.remove(&permit.proposal).ok_or(WorkError::ApprovalRequired)?;
        if !pending.approved || pending.revision != permit.revision || now_ms >= pending.expires_ms {
            return Err(WorkError::ApprovalRequired);
        }
        Ok(AuthorizedAction(pending.action))
    }

    pub fn source_policy_changed(&mut self) -> Result<(), WorkError> {
        self.policy_generation = next(self.policy_generation)?;
        self.pending.clear();
        Ok(())
    }

    /// Host-only, explicit approval of this full contribution set to this
    /// destination. Local read access and cloud-session opt-in are insufficient.
    pub fn approve_cloud_from_host(&self, destination: String, freshly_attested_sources: Vec<SourceRef>, now_ms: u64, ttl_ms: u64) -> Result<CloudPermit, WorkError> {
        text(&destination)?;
        if ttl_ms == 0 || ttl_ms > 60_000 {
            return Err(WorkError::DisclosureDenied);
        }
        self.check_contributions(&freshly_attested_sources)?;
        let expires_ms = now_ms.checked_add(ttl_ms).ok_or(WorkError::DisclosureDenied)?;
        Ok(CloudPermit { instance: Arc::clone(&self.instance), destination, revision: self.data.revision,
            policy_generation: self.policy_generation, expires_ms, sources: freshly_attested_sources })
    }

    fn check_contributions(&self, sources: &[SourceRef]) -> Result<(), WorkError> {
        let mut expected = self.data.sources.clone();
        // Task results also contribute; never omit their inherited policy.
        expected.extend(self.data.tasks.iter().filter_map(|t| t.artifact.clone()));
        expected.sort_by(|a, b| a.id.cmp(&b.id).then(a.version.cmp(&b.version)));
        expected.dedup();
        let mut actual = sources.to_vec();
        actual.sort_by(|a, b| a.id.cmp(&b.id).then(a.version.cmp(&b.version)));
        if actual != expected || actual.iter().any(|s| s.sensitivity == Sensitivity::Unknown) {
            return Err(WorkError::DisclosureDenied);
        }
        Ok(())
    }

    /// Re-attest immediately before egress, including ALL transitive sources
    /// for summaries. The host supplies live versions and live sensitivity.
    pub fn validate_cloud_release(&self, permit: &CloudPermit, destination: &str, freshly_attested_sources: &[SourceRef], now_ms: u64) -> Result<(), WorkError> {
        if !Arc::ptr_eq(&self.instance, &permit.instance) || permit.destination != destination
            || permit.revision != self.data.revision || permit.policy_generation != self.policy_generation
            || now_ms >= permit.expires_ms || permit.sources != freshly_attested_sources
        {
            return Err(WorkError::DisclosureDenied);
        }
        self.check_contributions(freshly_attested_sources)
    }
}

#[cfg(test)]
#[path = "work_tests.rs"]
mod tests;
