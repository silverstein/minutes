//! Work resumption, provenance-aware debriefs and steerable task records.
//!
//! Serialization produces data, never executable authority. Hosts choose their
//! existing policy-safe storage; importing a capsule never starts a process.

use serde::{Deserialize, Serialize};

use super::attention::SourceRef;
use super::{next, text, Result, MAX_ITEMS, MAX_SNAPSHOT_BYTES};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Observation,
    UserInterpretation,
    Suggestion,
    AcceptedDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub id: u64,
    pub kind: RecordKind,
    pub text: String,
    pub sources: Vec<SourceRef>,
    pub supersedes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkCapsule {
    version: u32,
    id: String,
    revision: u64,
    goal: String,
    constraints: Vec<String>,
    records: Vec<Record>,
    next_step: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CapsuleSnapshot {
    version: u32,
    id: String,
    revision: u64,
    goal: String,
    constraints: Vec<String>,
    records: Vec<Record>,
    next_step: Option<String>,
}

impl WorkCapsule {
    pub fn new(id: &str, goal: &str) -> Result<Self> {
        text(id)?;
        text(goal)?;
        Ok(Self {
            version: 1,
            id: id.into(),
            revision: 0,
            goal: goal.into(),
            constraints: Vec::new(),
            records: Vec::new(),
            next_step: None,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    pub fn goal(&self) -> &str {
        &self.goal
    }

    pub fn set_goal_from_host(&mut self, goal: &str, expected: u64) -> Result<()> {
        self.check_revision(expected)?;
        text(goal)?;
        let revision = next(self.revision)?;
        self.goal = goal.into();
        self.revision = revision;
        Ok(())
    }

    pub fn add_constraint_from_host(&mut self, value: &str, expected: u64) -> Result<()> {
        self.check_revision(expected)?;
        text(value)?;
        if self.constraints.len() >= MAX_ITEMS {
            return Err("constraint budget exceeded".into());
        }
        let revision = next(self.revision)?;
        self.constraints.push(value.into());
        self.revision = revision;
        Ok(())
    }

    pub fn propose(&mut self, value: &str, sources: Vec<SourceRef>, expected: u64) -> Result<u64> {
        self.append(RecordKind::Suggestion, value, sources, None, expected)
    }

    /// For host-attested observations. A model inference belongs in propose().
    pub fn record_observation_from_host(
        &mut self,
        value: &str,
        sources: Vec<SourceRef>,
        expected: u64,
    ) -> Result<u64> {
        if sources.is_empty() {
            return Err("observations require a source".into());
        }
        self.append(RecordKind::Observation, value, sources, None, expected)
    }

    /// Captures the user's private interpretation without rewriting a transcript.
    pub fn debrief_from_host(
        &mut self,
        value: &str,
        sources: Vec<SourceRef>,
        expected: u64,
    ) -> Result<u64> {
        self.append(
            RecordKind::UserInterpretation,
            value,
            sources,
            None,
            expected,
        )
    }

    pub fn accept_from_host(&mut self, suggestion: u64, expected: u64) -> Result<u64> {
        self.check_revision(expected)?;
        let record = self
            .records
            .iter()
            .find(|r| r.id == suggestion && r.kind == RecordKind::Suggestion)
            .ok_or("not a suggestion")?
            .clone();
        if self
            .records
            .iter()
            .any(|r| r.supersedes == Some(suggestion))
        {
            return Err("suggestion already accepted".into());
        }
        self.append(
            RecordKind::AcceptedDecision,
            &record.text,
            record.sources,
            Some(suggestion),
            expected,
        )
    }

    pub fn set_next_step(&mut self, value: &str, expected: u64) -> Result<()> {
        self.check_revision(expected)?;
        text(value)?;
        let revision = next(self.revision)?;
        self.next_step = Some(value.into());
        self.revision = revision;
        Ok(())
    }

    fn check_revision(&self, expected: u64) -> Result<()> {
        if expected != self.revision {
            return Err("work changed; refresh before updating".into());
        }
        Ok(())
    }

    fn append(
        &mut self,
        kind: RecordKind,
        value: &str,
        sources: Vec<SourceRef>,
        supersedes: Option<u64>,
        expected: u64,
    ) -> Result<u64> {
        self.check_revision(expected)?;
        text(value)?;
        if self.records.len() >= MAX_ITEMS || sources.len() > MAX_ITEMS {
            return Err("work record budget exceeded".into());
        }
        for source in &sources {
            source.validate()?;
        }
        let revision = next(self.revision)?;
        let used: usize = self
            .records
            .iter()
            .map(|r| {
                r.text.len()
                    + r.sources
                        .iter()
                        .map(|s| s.id.len() + s.snapshot.len())
                        .sum::<usize>()
            })
            .sum();
        let added = value.len()
            + sources
                .iter()
                .map(|s| s.id.len() + s.snapshot.len())
                .sum::<usize>();
        if used + added > MAX_SNAPSHOT_BYTES / 8 {
            return Err("work content budget exceeded".into());
        }
        self.records.push(Record {
            id: revision,
            kind,
            text: value.into(),
            sources,
            supersedes,
        });
        self.revision = revision;
        Ok(revision)
    }

    pub fn to_json(&self) -> Result<String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        if json.len() > MAX_SNAPSHOT_BYTES {
            return Err("capsule exceeds snapshot budget".into());
        }
        Ok(json)
    }

    /// Treat imported records as historical claims, not authority to act. Re-read
    /// every source with the current policy before exposing contents to a model.
    pub fn from_json(json: &str) -> Result<Self> {
        if json.len() > MAX_SNAPSHOT_BYTES {
            return Err("capsule exceeds snapshot budget".into());
        }
        let snapshot: CapsuleSnapshot = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let capsule = Self {
            version: snapshot.version,
            id: snapshot.id,
            revision: snapshot.revision,
            goal: snapshot.goal,
            constraints: snapshot.constraints,
            records: snapshot.records,
            next_step: snapshot.next_step,
        };
        text(&capsule.id)?;
        text(&capsule.goal)?;
        if capsule.version != 1
            || capsule.constraints.len() > MAX_ITEMS
            || capsule.records.len() > MAX_ITEMS
        {
            return Err("unsupported capsule or item budget exceeded".into());
        }
        for constraint in &capsule.constraints {
            text(constraint)?;
        }
        if let Some(step) = &capsule.next_step {
            text(step)?;
        }
        let mut previous = 0;
        let mut accepted = std::collections::BTreeSet::new();
        for record in &capsule.records {
            text(&record.text)?;
            if record.id <= previous
                || record.id > capsule.revision
                || record.sources.len() > MAX_ITEMS
                || (record.kind == RecordKind::Observation && record.sources.is_empty())
            {
                return Err("invalid work record".into());
            }
            for source in &record.sources {
                source.validate()?;
            }
            if record.kind == RecordKind::AcceptedDecision {
                let prior = capsule.records.iter().find(|r| {
                    Some(r.id) == record.supersedes
                        && r.id < record.id
                        && r.kind == RecordKind::Suggestion
                });
                if !accepted.insert(record.supersedes)
                    || prior.is_none_or(|r| r.text != record.text || r.sources != record.sources)
                {
                    return Err("decision has no matching prior suggestion".into());
                }
            } else if record.supersedes.is_some() {
                return Err("only accepted decisions supersede suggestions".into());
            }
            previous = record.id;
        }
        Ok(capsule)
    }

    /// JSON inside Markdown keeps the artifact portable without treating user
    /// strings as executable templates. No filesystem writes occur here.
    pub fn to_markdown(&self) -> Result<String> {
        Ok(format!(
            "# Work capsule\n\nLocal history, not an execution grant.\n\n```json\n{}\n```\n",
            self.to_json()?
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Prepared,
    Running,
    CancelRequested,
    Cancelled,
    Completed,
    Failed,
    NeedsReconciliation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    pub workspace: String,
    pub instruction: String,
    /// An adapter must enforce this allowlist; a prompt saying read-only is not enforcement.
    pub allowed_tools: Vec<String>,
}

impl TaskSpec {
    fn validate(&self) -> Result<()> {
        text(&self.workspace)?;
        text(&self.instruction)?;
        if self.allowed_tools.is_empty() || self.allowed_tools.len() > MAX_ITEMS {
            return Err("task requires a bounded tool allowlist".into());
        }
        for tool in &self.allowed_tools {
            text(tool)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunKey {
    pub task_id: String,
    pub generation: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Task {
    id: String,
    generation: u64,
    spec: TaskSpec,
    state: TaskState,
    pending_instruction: Option<String>,
    receipt: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskSnapshot {
    id: String,
    generation: u64,
    spec: TaskSpec,
    state: TaskState,
    pending_instruction: Option<String>,
    receipt: Option<String>,
}

impl Task {
    pub fn new(id: &str, spec: TaskSpec) -> Result<Self> {
        text(id)?;
        spec.validate()?;
        Ok(Self {
            id: id.into(),
            generation: 0,
            spec,
            state: TaskState::Prepared,
            pending_instruction: None,
            receipt: None,
        })
    }

    pub fn state(&self) -> &TaskState {
        &self.state
    }

    pub fn spec(&self) -> &TaskSpec {
        &self.spec
    }

    /// Host-controlled dispatch. This does not itself launch an agent.
    pub fn start_from_host(&mut self) -> Result<RunKey> {
        if self.state != TaskState::Prepared {
            return Err("task is not ready for explicit dispatch".into());
        }
        self.generation = next(self.generation)?;
        self.state = TaskState::Running;
        Ok(RunKey {
            task_id: self.id.clone(),
            generation: self.generation,
        })
    }

    /// Steering never starts overlapping work. Deliver cancellation to the
    /// adapter; only its acknowledgement permits another generation to run.
    pub fn steer_from_host(&mut self, instruction: &str) -> Result<RunKey> {
        text(instruction)?;
        let key = self.cancel_from_host()?;
        self.pending_instruction = Some(instruction.into());
        Ok(key)
    }

    pub fn cancel_from_host(&mut self) -> Result<RunKey> {
        if self.state != TaskState::Running && self.state != TaskState::CancelRequested {
            return Err("task is not running".into());
        }
        self.state = TaskState::CancelRequested;
        self.pending_instruction = None;
        Ok(RunKey {
            task_id: self.id.clone(),
            generation: self.generation,
        })
    }

    pub fn cancellation_acknowledged(&mut self, key: &RunKey, stopped: bool) -> Result<()> {
        self.check_key(key)?;
        if self.state != TaskState::CancelRequested {
            return Err("no cancellation is pending".into());
        }
        if !stopped {
            self.state = TaskState::NeedsReconciliation;
            return Ok(());
        }
        self.state = if let Some(instruction) = self.pending_instruction.take() {
            self.spec.instruction = instruction;
            TaskState::Prepared
        } else {
            TaskState::Cancelled
        };
        Ok(())
    }

    pub fn finish(&mut self, key: &RunKey, success: bool, receipt: &str) -> Result<()> {
        self.check_key(key)?;
        text(receipt)?;
        if self.state != TaskState::Running {
            return Err("completion requires reconciliation or is stale".into());
        }
        self.receipt = Some(receipt.into());
        self.state = if success {
            TaskState::Completed
        } else {
            TaskState::Failed
        };
        Ok(())
    }

    fn check_key(&self, key: &RunKey) -> Result<()> {
        if key.task_id != self.id || key.generation != self.generation {
            return Err("completion belongs to a different task generation".into());
        }
        Ok(())
    }

    pub fn to_json(&self) -> Result<String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        if json.len() > MAX_SNAPSHOT_BYTES {
            return Err("task exceeds snapshot budget".into());
        }
        Ok(json)
    }

    /// An uncertain in-flight task is never auto-replayed after a restart.
    pub fn restore(json: &str) -> Result<Self> {
        if json.len() > MAX_SNAPSHOT_BYTES {
            return Err("task exceeds snapshot budget".into());
        }
        let snapshot: TaskSnapshot = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let mut task = Self {
            id: snapshot.id,
            generation: snapshot.generation,
            spec: snapshot.spec,
            state: snapshot.state,
            pending_instruction: snapshot.pending_instruction,
            receipt: snapshot.receipt,
        };
        text(&task.id)?;
        task.spec.validate()?;
        if let Some(instruction) = &task.pending_instruction {
            text(instruction)?;
        }
        if let Some(receipt) = &task.receipt {
            text(receipt)?;
        }
        if matches!(task.state, TaskState::Completed | TaskState::Failed)
            && (task.receipt.is_none() || task.generation == 0)
        {
            return Err("finished task has no execution receipt".into());
        }
        if matches!(task.state, TaskState::Running | TaskState::CancelRequested) {
            task.state = TaskState::NeedsReconciliation;
        }
        Ok(task)
    }
}
