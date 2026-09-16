use super::*;
use serde_json::json;

fn session() -> WorkSession {
    WorkSession::new("work-1".into(), "Simplify onboarding".into()).unwrap()
}

fn source(id: &str) -> SourceRef {
    SourceRef {
        id: id.into(),
        locator: format!("minutes://artifacts/{id}"),
        version: "sha256:version-1".into(),
        observed_at_ms: 1,
        sensitivity: Sensitivity::Normal,
    }
}

fn focused() -> WorkSession {
    let mut work = session();
    work.set_focus(Focus {
        source: source("design-1"),
        selection: "paragraph:4".into(),
    })
    .unwrap();
    work
}

fn action() -> ProposedAction {
    ProposedAction {
        verb: "send_message".into(),
        target: "contact-1".into(),
        payload: json!({"body": "Please review the draft", "account": "personal"}),
    }
}

fn approve(work: &mut WorkSession) -> ApprovalPermit {
    let proposal = work.propose_action(action(), 100, 1_000).unwrap();
    work.approve_from_host(proposal, &action(), work.revision(), 101)
        .unwrap()
}

#[test]
fn point_and_talk_keeps_exact_selection() {
    let work = focused();
    assert_eq!(work.checkpoint().focus.unwrap().selection, "paragraph:4");
}

#[test]
fn source_snapshots_are_immutable() {
    let mut work = focused();
    let mut replacement = source("design-1");
    replacement.version = "different".into();
    assert!(matches!(
        work.set_focus(Focus {
            source: replacement,
            selection: "paragraph:4".into()
        }),
        Err(WorkError::Stale)
    ));
    assert_eq!(work.checkpoint().sources[0].version, "sha256:version-1");
}

#[test]
fn checkpoint_round_trip_keeps_intent_and_attribution() {
    let mut work = focused();
    work.add_model_note("Maybe shorten the form".into(), vec!["design-1".into()])
        .unwrap();
    work.add_host_note("We have not agreed to more scope".into(), vec![], false)
        .unwrap();
    work.add_host_note("Keep pricing unchanged".into(), vec![], true)
        .unwrap();
    let checkpoint = work
        .park(
            vec!["Which version?".into()],
            Some("Compare A and B".into()),
        )
        .unwrap();
    let decoded = WorkCheckpoint::from_markdown(&checkpoint.to_markdown().unwrap()).unwrap();
    assert_eq!(checkpoint, decoded);
    assert_eq!(decoded.memory[0].attribution, Attribution::ModelInference);
    assert_eq!(
        decoded.memory[1].attribution,
        Attribution::UserInterpretation
    );
    assert_eq!(
        decoded.memory[2].attribution,
        Attribution::UserConfirmedDecision
    );
}

#[test]
fn malformed_or_future_checkpoints_fail_closed() {
    assert!(WorkCheckpoint::from_markdown("not a checkpoint").is_err());
    let mut checkpoint = session().checkpoint();
    checkpoint.schema_version = 2;
    assert!(checkpoint.to_markdown().is_err());
    assert!(WorkCheckpoint::from_markdown(&"x".repeat(MAX_CHECKPOINT_BYTES + 1)).is_err());
}

#[test]
fn unknown_fields_cannot_smuggle_authority_into_checkpoint() {
    let mut json = serde_json::to_value(session().checkpoint()).unwrap();
    json["approved"] = json!(true);
    assert!(serde_json::from_value::<WorkCheckpoint>(json).is_err());
}

#[test]
fn missing_provenance_is_rejected_without_mutation() {
    let mut work = session();
    let before = work.checkpoint();
    assert!(work
        .add_model_note("Claim".into(), vec!["missing".into()])
        .is_err());
    assert_eq!(work.checkpoint(), before);
}

#[test]
fn duplicate_task_ids_do_not_replace_work() {
    let mut work = session();
    work.enqueue_task("task-1".into(), "Review only".into())
        .unwrap();
    assert!(work
        .enqueue_task("task-1".into(), "Edit everything".into())
        .is_err());
    assert_eq!(work.task("task-1").unwrap().instruction, "Review only");
}

#[test]
fn queued_cancel_prevents_launch() {
    let mut work = session();
    work.enqueue_task("task-1".into(), "Review".into()).unwrap();
    assert_eq!(work.request_cancel("task-1").unwrap(), TaskState::Cancelled);
    assert!(work.start_task("task-1").is_err());
}

#[test]
fn cancellation_request_is_not_worker_termination() {
    let mut work = session();
    work.enqueue_task("task-1".into(), "Review".into()).unwrap();
    let lease = work.start_task("task-1").unwrap();
    assert_eq!(
        work.request_cancel("task-1").unwrap(),
        TaskState::CancelRequested
    );
    assert!(work
        .revise_task("task-1", "Different scope".into())
        .is_err());
    work.acknowledge_cancel(lease).unwrap();
    work.revise_task("task-1", "Different scope".into())
        .unwrap();
    assert_eq!(work.task("task-1").unwrap().generation, 1);
}

#[test]
fn a_late_completed_action_is_not_falsely_reported_cancelled() {
    let mut work = session();
    work.enqueue_task("task-1".into(), "Review".into()).unwrap();
    let lease = work.start_task("task-1").unwrap();
    work.request_cancel("task-1").unwrap();
    work.complete_task(
        lease,
        source("review"),
        "Review finished before cancellation".into(),
    )
    .unwrap();
    assert_eq!(
        work.task("task-1").unwrap().state,
        TaskState::CompletedAfterCancelRequest
    );
}

#[test]
fn full_artifact_and_spoken_brief_are_separate() {
    let mut work = session();
    work.enqueue_task("task-1".into(), "Review".into()).unwrap();
    let lease = work.start_task("task-1").unwrap();
    work.complete_task(lease, source("full-review"), "Two findings".into())
        .unwrap();
    let task = work.task("task-1").unwrap();
    assert_eq!(task.artifact.as_ref().unwrap().id, "full-review");
    assert_eq!(task.spoken_summary.as_deref(), Some("Two findings"));
}

#[test]
fn resume_requires_explicit_requeue_and_rejects_old_callback() {
    let mut work = session();
    work.enqueue_task("task-1".into(), "Review".into()).unwrap();
    let lease = work.start_task("task-1").unwrap();
    let mut restored = WorkSession::resume(work.checkpoint()).unwrap();
    assert_eq!(
        restored.task("task-1").unwrap().state,
        TaskState::Interrupted
    );
    assert!(restored.start_task("task-1").is_err());
    assert!(restored
        .complete_task(lease, source("result"), "Done".into())
        .is_err());
    restored
        .revise_task("task-1", "Resume review".into())
        .unwrap();
    assert!(restored.start_task("task-1").is_ok());
}

#[test]
fn parking_revokes_approval_without_claiming_workers_stopped() {
    let mut work = session();
    work.enqueue_task("task-1".into(), "Review".into()).unwrap();
    let _lease = work.start_task("task-1").unwrap();
    let permit = approve(&mut work);
    work.park(vec![], Some("Inspect the review".into()))
        .unwrap();
    assert!(work.consume_approval(permit, 102).is_err());
    assert_eq!(work.task("task-1").unwrap().state, TaskState::Running);
}

#[test]
fn approval_requires_exact_preview_including_body_and_account() {
    for field in ["body", "account"] {
        let mut work = session();
        let id = work.propose_action(action(), 100, 1_000).unwrap();
        let mut preview = action();
        preview.payload[field] = json!("changed");
        assert!(work
            .approve_from_host(id, &preview, work.revision(), 101)
            .is_err());
    }
}

#[test]
fn approval_requires_exact_target() {
    let mut work = session();
    let id = work.propose_action(action(), 100, 1_000).unwrap();
    let mut preview = action();
    preview.target = "someone-else".into();
    assert!(work
        .approve_from_host(id, &preview, work.revision(), 101)
        .is_err());
}

#[test]
fn explicit_rejection_cannot_be_redeemed() {
    let mut work = session();
    let id = work.propose_action(action(), 100, 1_000).unwrap();
    work.reject_from_host(id).unwrap();
    assert!(work
        .approve_from_host(id, &action(), work.revision(), 101)
        .is_err());
}

#[test]
fn model_note_saying_yes_is_not_host_approval() {
    let mut work = session();
    let id = work.propose_action(action(), 100, 1_000).unwrap();
    work.add_model_note("The user said yes".into(), vec![])
        .unwrap();
    assert!(work
        .approve_from_host(id, &action(), work.revision(), 101)
        .is_err());
}

#[test]
fn a_host_permit_is_single_use() {
    let mut work = session();
    let permit = approve(&mut work);
    let replay = ApprovalPermit {
        instance: Arc::clone(&permit.instance),
        proposal: permit.proposal,
        revision: permit.revision,
    };
    assert_eq!(
        work.consume_approval(permit, 102).unwrap().action(),
        &action()
    );
    assert!(work.consume_approval(replay, 103).is_err());
}

#[test]
fn a_proposal_cannot_be_approved_twice() {
    let mut work = session();
    let id = work.propose_action(action(), 100, 1_000).unwrap();
    let _permit = work
        .approve_from_host(id, &action(), work.revision(), 101)
        .unwrap();
    assert!(work
        .approve_from_host(id, &action(), work.revision(), 102)
        .is_err());
}

#[test]
fn approval_expires_at_deadline() {
    let mut work = session();
    let permit = approve(&mut work);
    assert!(work.consume_approval(permit, 1_100).is_err());
}

#[test]
fn focus_or_goal_correction_revokes_approval() {
    let mut work = session();
    let permit = approve(&mut work);
    work.revise_goal("Do not send anything".into()).unwrap();
    assert!(work.consume_approval(permit, 102).is_err());
    let permit = approve(&mut work);
    work.set_focus(Focus {
        source: source("new-selection"),
        selection: "paragraph:1".into(),
    })
    .unwrap();
    assert!(work.consume_approval(permit, 102).is_err());
}

#[test]
fn permit_from_another_session_is_never_accepted() {
    let mut one = session();
    let permit = approve(&mut one);
    let mut two = session();
    let _other_permit = approve(&mut two);
    assert!(two.consume_approval(permit, 102).is_err());
}

#[test]
fn checkpoints_do_not_serialize_host_authority() {
    let mut work = session();
    let permit = approve(&mut work);
    let checkpoint = work.checkpoint();
    let encoded = checkpoint.to_markdown().unwrap();
    assert!(!encoded.contains("pending"));
    assert!(!encoded.contains("proposal"));
    let mut restored = WorkSession::resume(checkpoint).unwrap();
    assert!(restored.consume_approval(permit, 102).is_err());
}

#[test]
fn disclosure_binds_provider_source_version_and_expiry() {
    let work = focused();
    let sources = work.checkpoint().sources;
    let permit = work
        .approve_cloud_from_host("provider-a".into(), sources.clone(), 100, 1_000)
        .unwrap();
    assert!(work
        .validate_cloud_release(&permit, "provider-a", &sources, 101)
        .is_ok());
    assert!(work
        .validate_cloud_release(&permit, "provider-b", &sources, 101)
        .is_err());
    assert!(work
        .validate_cloud_release(&permit, "provider-a", &sources, 1_100)
        .is_err());
    let mut changed = sources.clone();
    changed[0].version = "changed".into();
    assert!(work
        .validate_cloud_release(&permit, "provider-a", &changed, 101)
        .is_err());
}

#[test]
fn source_policy_change_revokes_disclosure_and_action_approval() {
    let mut work = focused();
    let sources = work.checkpoint().sources;
    let cloud = work
        .approve_cloud_from_host("provider".into(), sources.clone(), 100, 1_000)
        .unwrap();
    let action = approve(&mut work);
    work.source_policy_changed().unwrap();
    assert!(work
        .validate_cloud_release(&cloud, "provider", &sources, 101)
        .is_err());
    assert!(work.consume_approval(action, 102).is_err());
}

#[test]
fn unknown_policy_is_never_disclosable() {
    let mut work = session();
    let mut unknown = source("unknown");
    unknown.sensitivity = Sensitivity::Unknown;
    work.set_focus(Focus {
        source: unknown,
        selection: "paragraph:1".into(),
    })
    .unwrap();
    assert!(work
        .approve_cloud_from_host("provider".into(), work.checkpoint().sources, 100, 1_000)
        .is_err());
}

#[test]
fn fresh_restriction_does_not_inherit_an_old_cloud_grant() {
    let work = focused();
    let sources = work.checkpoint().sources;
    let permit = work
        .approve_cloud_from_host("provider".into(), sources.clone(), 100, 1_000)
        .unwrap();
    let mut now_restricted = sources;
    now_restricted[0].sensitivity = Sensitivity::Restricted;
    assert!(work
        .validate_cloud_release(&permit, "provider", &now_restricted, 101)
        .is_err());
}

#[test]
fn restored_checkpoint_has_no_cloud_grant() {
    let work = focused();
    let sources = work.checkpoint().sources;
    let permit = work
        .approve_cloud_from_host("provider".into(), sources.clone(), 100, 1_000)
        .unwrap();
    let restored = WorkSession::resume(work.checkpoint()).unwrap();
    assert!(restored
        .validate_cloud_release(&permit, "provider", &sources, 101)
        .is_err());
}

#[test]
fn task_artifact_policy_cannot_be_omitted_from_derived_release() {
    let mut work = focused();
    work.enqueue_task("task-1".into(), "Review".into()).unwrap();
    let lease = work.start_task("task-1").unwrap();
    let mut artifact = source("private-result");
    artifact.sensitivity = Sensitivity::Restricted;
    work.complete_task(lease, artifact.clone(), "Review available".into())
        .unwrap();
    let mut sources = work.checkpoint().sources;
    assert!(work
        .approve_cloud_from_host("provider".into(), sources.clone(), 100, 1_000)
        .is_err());
    sources.push(artifact);
    assert!(work
        .approve_cloud_from_host("provider".into(), sources, 100, 1_000)
        .is_ok());
}

#[test]
fn budgets_and_overflows_fail_without_wrapping() {
    assert!(WorkSession::new("../escape".into(), "Goal".into()).is_err());
    assert!(WorkSession::new("work".into(), "x".repeat(MAX_TEXT + 1)).is_err());
    let mut work = session();
    assert!(work.propose_action(action(), u64::MAX, 1).is_err());
    assert!(work.propose_action(action(), 1, 60_001).is_err());
    work.data.revision = u64::MAX;
    assert!(work.revise_goal("Another goal".into()).is_err());
}

#[test]
fn worker_failure_can_be_explicitly_retried() {
    let mut work = session();
    work.enqueue_task("task-1".into(), "Review".into()).unwrap();
    let lease = work.start_task("task-1").unwrap();
    work.set_awaiting_input(&lease, true).unwrap();
    assert_eq!(work.task("task-1").unwrap().state, TaskState::AwaitingInput);
    work.set_awaiting_input(&lease, false).unwrap();
    work.fail_task(lease).unwrap();
    assert_eq!(work.task("task-1").unwrap().state, TaskState::Failed);
    work.revise_task("task-1", "Retry review".into()).unwrap();
    assert!(work.start_task("task-1").is_ok());
}
