use serde_json::json;

use super::attention::*;
use super::authority::*;
use super::live::*;
use super::work::*;

fn target() -> Target {
    Target {
        application: "editor".into(),
        window: "window-1".into(),
        artifact: Some("proposal".into()),
    }
}

fn packet(request: CaptureRequest) -> ContextPacket {
    ContextPacket {
        observed_at_ms: request.requested_at_ms,
        observed_target: request.target.clone(),
        request,
        selected_text: Some("the selected paragraph".into()),
        image_ref: None,
    }
}

fn source() -> SourceRef {
    SourceRef {
        id: "meeting-1".into(),
        snapshot: "digest-of-observed-bytes".into(),
        policy_generation: 3,
        sensitivity: Sensitivity::Normal,
    }
}

fn action() -> Action {
    Action {
        operation: "send_email".into(),
        account: "work".into(),
        recipient_or_target: "fixture@example.invalid".into(),
        payload: "Exact body. Attachments: none.".into(),
    }
}

fn task() -> Task {
    Task::new(
        "task-1",
        TaskSpec {
            workspace: "fixture-workspace".into(),
            instruction: "Review; do not change anything".into(),
            allowed_tools: vec!["read_file".into()],
        },
    )
    .unwrap()
}

#[test]
fn no_implicit_attention_or_capture() {
    let mut session = AttentionSession::default();
    assert!(session.request(10).is_err());
}

#[test]
fn point_and_talk_consumes_the_exact_snapshot_once() {
    let mut session = AttentionSession::default();
    session.begin(target(), 10, 1000).unwrap();
    let frame = packet(session.request(20).unwrap());
    assert!(session.accept(frame.clone(), 25).is_ok());
    assert!(session.accept(frame, 25).is_err());
}

#[test]
fn switched_window_cannot_masquerade_as_requested_window() {
    let mut session = AttentionSession::default();
    session.begin(target(), 10, 1000).unwrap();
    let mut frame = packet(session.request(20).unwrap());
    frame.observed_target.window = "overlay".into();
    assert!(session.accept(frame, 25).is_err());
}

#[test]
fn newer_request_invalidates_in_flight_frame() {
    let mut session = AttentionSession::default();
    session.begin(target(), 10, 1000).unwrap();
    let old = packet(session.request(20).unwrap());
    let new = packet(session.request(30).unwrap());
    assert!(session.accept(old, 35).is_err());
    assert!(session.accept(new, 35).is_ok());
}

#[test]
fn expiry_stop_future_and_stale_frames_fail_closed() {
    for now in [19, 1020, 10000] {
        let mut session = AttentionSession::default();
        session.begin(target(), 10, 1000).unwrap();
        let frame = packet(session.request(20).unwrap());
        assert!(session.accept(frame, now).is_err());
    }
    let mut session = AttentionSession::default();
    session.begin(target(), 10, 10000).unwrap();
    let frame = packet(session.request(20).unwrap());
    assert!(session.accept(frame.clone(), 5021).is_err());
    session.stop();
    assert!(session.accept(frame, 25).is_err());
}

#[test]
fn context_limits_are_enforced() {
    let mut session = AttentionSession::default();
    assert!(session.begin(target(), 0, 0).is_err());
    assert!(session.begin(target(), u64::MAX, 10).is_err());
    session.begin(target(), 0, 100).unwrap();
    let mut frame = packet(session.request(1).unwrap());
    frame.selected_text = Some("x".repeat(super::MAX_TEXT_BYTES + 1));
    assert!(session.accept(frame, 2).is_err());
}

#[test]
fn cloud_opt_in_is_not_a_restricted_override() {
    for sensitivity in [Sensitivity::Restricted, Sensitivity::Unknown] {
        let mut s = source();
        s.sensitivity = sensitivity;
        assert!(validate_cloud_release(
            true,
            std::slice::from_ref(&s),
            std::slice::from_ref(&s)
        )
        .is_err());
    }
    assert!(validate_cloud_release(false, &[source()], &[source()]).is_err());
    assert!(validate_cloud_release(true, &[source()], &[source()]).is_ok());
}

#[test]
fn source_changes_and_duplicate_attestations_are_rejected() {
    let mut changed = source();
    changed.policy_generation += 1;
    assert!(validate_cloud_release(true, &[source()], &[changed]).is_err());
    let mut changed = source();
    changed.snapshot = "new-bytes".into();
    assert!(validate_cloud_release(true, &[source()], &[changed]).is_err());
    assert!(validate_cloud_release(true, &[source()], &[]).is_err());
    assert!(validate_cloud_release(true, &[source(), source()], &[source(), source()]).is_err());
}

#[test]
fn knowing_proposal_id_or_hearing_another_utterance_is_not_approval() {
    let mut gate = ApprovalGate::default();
    let proposal = gate.propose(action(), 10, 100, 1).unwrap();
    for _utterance in ["no", "wait", "wrong person", "yes", "unrelated speech"] {
        // Transcripts are evidence. There is intentionally no transcript-to-approval API.
        assert!(gate.take_approved(proposal.id, 20, 1).is_err());
    }
    gate.approve_from_host(&proposal, 20, 1).unwrap();
    assert_eq!(gate.take_approved(proposal.id, 21, 1).unwrap(), action());
    assert!(gate.take_approved(proposal.id, 22, 1).is_err());
}

#[test]
fn altered_review_cannot_authorize_an_action() {
    let mut gate = ApprovalGate::default();
    let mut review = gate.propose(action(), 10, 100, 1).unwrap();
    review.action.payload = "Different body".into();
    assert!(gate.approve_from_host(&review, 20, 1).is_err());
}

#[test]
fn correction_rejection_and_policy_changes_revoke_authority() {
    let mut gate = ApprovalGate::default();
    let old = gate.propose(action(), 10, 100, 1).unwrap();
    gate.approve_from_host(&old, 20, 1).unwrap();
    let new = gate.propose(action(), 21, 100, 1).unwrap();
    assert!(gate.take_approved(old.id, 22, 1).is_err());
    gate.approve_from_host(&new, 22, 1).unwrap();
    assert!(gate.take_approved(new.id, 23, 2).is_err());
    gate.reject_from_host();
    assert!(gate.take_approved(new.id, 23, 1).is_err());
}

#[test]
fn expired_or_regressed_clock_approval_is_rejected() {
    let mut gate = ApprovalGate::default();
    let proposal = gate.propose(action(), 10, 100, 1).unwrap();
    assert!(gate.approve_from_host(&proposal, 9, 1).is_err());
    assert!(gate.approve_from_host(&proposal, 110, 1).is_err());
}

#[test]
fn extended_thinking_uses_async_tools_and_omits_scheduling() {
    let profile = LiveProfile::gemini("gemini-3.8-live-extended-thinking").unwrap();
    let decl = profile
        .function_declaration(json!({"name": "read", "behavior": "BLOCKING"}))
        .unwrap();
    assert_eq!(decl["behavior"], "NON_BLOCKING");
    let response = profile
        .function_response("1", "read", json!("result"), "INTERRUPT")
        .unwrap();
    assert!(response["response"].get("scheduling").is_none());
    assert!(profile.proactive_audio(false));
}

#[test]
fn standard_profile_retains_scheduling_and_unknown_models_are_explicit() {
    let profile = LiveProfile::gemini("models/gemini-3.8-live").unwrap();
    let response = profile
        .function_response("1", "read", json!("ok"), "WHEN_IDLE")
        .unwrap();
    assert_eq!(response["response"]["scheduling"], "WHEN_IDLE");
    assert!(LiveProfile::gemini("future-model").is_err());
    assert!(profile
        .function_response("1", "read", json!(null), "invalid")
        .is_err());
}

#[test]
fn reasoning_is_not_idle_after_turn_complete_or_playback_stop() {
    let mut activity = LiveActivity::new(LiveProfile::AsyncReasoning);
    activity.user_input_started();
    activity.user_input_finished();
    activity.playback_started();
    activity.turn_complete();
    activity.playback_stopped();
    assert!(!activity.ready());
    activity.provider_status(ProviderActivity::Idle);
    assert!(activity.ready());
}

#[test]
fn silence_and_disconnect_do_not_cancel_tools() {
    let mut activity = LiveActivity::new(LiveProfile::AsyncReasoning);
    activity.tool_started("call-1").unwrap();
    assert!(activity.tool_started("call-1").is_err());
    activity.playback_stopped();
    activity.disconnected();
    assert_eq!(activity.pending_tools(), 1);
    assert!(!activity.ready());
    activity.provider_status(ProviderActivity::Idle);
    assert!(!activity.ready());
    activity.tool_finished("call-1").unwrap();
    assert!(activity.ready());
}

#[test]
fn interaction_status_accepts_wire_spellings_and_fails_closed_on_unknown() {
    for frame in [
        json!({"interactionStatus":"IDLE"}),
        json!({"interaction_status":"IDLE"}),
    ] {
        assert_eq!(interaction_status(&frame), Some(ProviderActivity::Idle));
    }
    assert_eq!(
        interaction_status(&json!({"interactionStatus":"FUTURE"})),
        Some(ProviderActivity::Unknown)
    );
    assert_eq!(interaction_status(&json!({})), None);
}

#[test]
fn debrief_does_not_rewrite_observation_or_promote_an_idea() {
    let mut capsule = WorkCapsule::new("work-1", "Simplify the offer").unwrap();
    capsule
        .record_observation_from_host("We discussed expanded scope", vec![source()], 0)
        .unwrap();
    capsule
        .debrief_from_host("I was interested, but did not agree", vec![source()], 1)
        .unwrap();
    let suggestion = capsule
        .propose("Keep the current scope", vec![source()], 2)
        .unwrap();
    assert_eq!(capsule.records()[0].kind, RecordKind::Observation);
    assert_eq!(capsule.records()[1].kind, RecordKind::UserInterpretation);
    assert_eq!(capsule.records()[2].kind, RecordKind::Suggestion);
    assert!(capsule.accept_from_host(suggestion, 2).is_err());
    capsule.accept_from_host(suggestion, 3).unwrap();
    assert_eq!(capsule.records()[3].kind, RecordKind::AcceptedDecision);
    assert!(capsule.accept_from_host(suggestion, 4).is_err());
}

#[test]
fn capsule_roundtrip_and_stale_updates_are_safe() {
    let mut capsule = WorkCapsule::new("work-1", "Review the proposal").unwrap();
    capsule.add_constraint_from_host("Keep pricing", 0).unwrap();
    capsule.set_next_step("Compare the two versions", 1).unwrap();
    let restored = WorkCapsule::from_json(&capsule.to_json().unwrap()).unwrap();
    assert_eq!(restored.goal(), "Review the proposal");
    assert_eq!(restored.revision(), 2);
    assert!(capsule.set_goal_from_host("Stale goal", 1).is_err());
    assert!(capsule
        .to_markdown()
        .unwrap()
        .contains("not an execution grant"));
}

#[test]
fn capsule_rejects_future_versions_and_invalid_decision_history() {
    let capsule = WorkCapsule::new("work-1", "Review").unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&capsule.to_json().unwrap()).unwrap();
    value["version"] = json!(2);
    assert!(WorkCapsule::from_json(&value.to_string()).is_err());
    value["version"] = json!(1);
    value["revision"] = json!(1);
    value["records"] = json!([{"id":1,"kind":"accepted_decision","text":"Do it","sources":[],"supersedes":99}]);
    assert!(WorkCapsule::from_json(&value.to_string()).is_err());
}

#[test]
fn steering_waits_for_cancellation_and_rejects_old_completion() {
    let mut task = task();
    let old = task.start_from_host().unwrap();
    let cancel = task.steer_from_host("Review only authorization").unwrap();
    assert_eq!(old, cancel);
    assert!(task.start_from_host().is_err());
    task.cancellation_acknowledged(&cancel, true).unwrap();
    let new = task.start_from_host().unwrap();
    assert_eq!(task.spec().instruction, "Review only authorization");
    assert!(task.finish(&old, true, "stale result").is_err());
    task.finish(&new, true, "review artifact reference").unwrap();
    assert_eq!(task.state(), &TaskState::Completed);
}

#[test]
fn restart_never_auto_replays_uncertain_work() {
    let mut task = task();
    task.start_from_host().unwrap();
    let mut restored = Task::restore(&task.to_json().unwrap()).unwrap();
    assert_eq!(restored.state(), &TaskState::NeedsReconciliation);
    assert!(restored.start_from_host().is_err());
}

#[test]
fn failed_cancellation_requires_reconciliation_not_a_success_message() {
    let mut task = task();
    task.start_from_host().unwrap();
    let key = task.cancel_from_host().unwrap();
    task.cancellation_acknowledged(&key, false).unwrap();
    assert_eq!(task.state(), &TaskState::NeedsReconciliation);
    assert!(task.finish(&key, true, "late").is_err());
}

#[test]
fn source_less_facts_and_missing_execution_receipts_are_rejected() {
    let mut capsule = WorkCapsule::new("work-1", "Review").unwrap();
    assert!(capsule
        .record_observation_from_host("A fact", vec![], 0)
        .is_err());
    let mut value: serde_json::Value = serde_json::from_str(&task().to_json().unwrap()).unwrap();
    value["state"] = json!("completed");
    assert!(Task::restore(&value.to_string()).is_err());
}
