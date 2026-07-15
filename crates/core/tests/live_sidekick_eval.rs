use minutes_core::live_sidekick::{
    AssistanceEvent, AssistancePosture, AssistanceSurface, BackgroundRunId, CaptureMode,
    CaptureSessionId, EvidenceId, EvidenceSourceKind, ForegroundTurnId, LiveAssistanceSession,
    LiveAssistanceSessionId, MeetingRef, Reduction, UntrustedEvidence, UserRole,
};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/live_sidekick_eval/v1")
}

fn required_str<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string field {key}"))
}

fn payload_str<'a>(event: &'a Value, key: &str) -> Result<&'a str, String> {
    event
        .get("payload")
        .and_then(|payload| payload.get(key))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing payload string {key}"))
}

fn parse_surface(value: &str) -> Result<AssistanceSurface, String> {
    match value {
        "terminal" => Ok(AssistanceSurface::TerminalSidekick),
        "gui" => Ok(AssistanceSurface::NativeRecall),
        "coach" => Ok(AssistanceSurface::CoachHud),
        other => Err(format!("unsupported surface {other}")),
    }
}

fn parse_role(value: &str) -> Result<UserRole, String> {
    match value {
        "presenter" => Ok(UserRole::Presenter),
        "participant" => Ok(UserRole::Participant),
        "observer" => Ok(UserRole::Observer),
        "decision_maker" => Ok(UserRole::DecisionMaker),
        "technical_responder" => Ok(UserRole::TechnicalResponder),
        other => Err(format!("unsupported user role {other}")),
    }
}

fn parse_posture(value: &str) -> Result<AssistancePosture, String> {
    match value {
        "on_demand" => Ok(AssistancePosture::OnDemand),
        "strategist" => Ok(AssistancePosture::Strategist),
        "silent_watch" => Ok(AssistancePosture::SilentWatch),
        "decision_tracker" => Ok(AssistancePosture::DecisionTracker),
        other => Err(format!("unsupported posture {other}")),
    }
}

fn parse_capture_mode(value: &str) -> Result<CaptureMode, String> {
    match value {
        "live" => Ok(CaptureMode::Live),
        "recording" => Ok(CaptureMode::Recording),
        other => Err(format!("unsupported capture mode {other}")),
    }
}

fn translate_event(event: &Value) -> Result<AssistanceEvent, String> {
    let kind = required_str(event, "kind")?;
    let session_id = LiveAssistanceSessionId::new(required_str(event, "session_id")?);
    let payload = event
        .get("payload")
        .and_then(Value::as_object)
        .ok_or_else(|| "missing payload object".to_string())?;

    match kind {
        "capture_started" => Ok(AssistanceEvent::CaptureStarted {
            session_id,
            capture_session_id: CaptureSessionId::new(payload_str(event, "capture_session_id")?),
            mode: parse_capture_mode(payload_str(event, "capture_mode")?)?,
        }),
        "transcript_final" => Ok(AssistanceEvent::EvidenceObserved {
            session_id,
            evidence: UntrustedEvidence {
                id: EvidenceId::new(payload_str(event, "event_id")?),
                source_kind: EvidenceSourceKind::TranscriptFinal,
                capture_session_id: Some(CaptureSessionId::new(payload_str(
                    event,
                    "capture_session_id",
                )?)),
                finalized_meeting_ref: None,
            },
        }),
        "screen_disclosed" => Ok(AssistanceEvent::EvidenceObserved {
            session_id,
            evidence: UntrustedEvidence {
                id: EvidenceId::new(payload_str(event, "event_id")?),
                source_kind: EvidenceSourceKind::ScreenImage,
                capture_session_id: Some(CaptureSessionId::new(payload_str(
                    event,
                    "capture_session_id",
                )?)),
                finalized_meeting_ref: None,
            },
        }),
        "coach_nudge" => Ok(AssistanceEvent::EvidenceObserved {
            session_id,
            evidence: UntrustedEvidence {
                id: EvidenceId::new(payload_str(event, "event_id")?),
                source_kind: EvidenceSourceKind::CoachNudge,
                capture_session_id: Some(CaptureSessionId::new(payload_str(
                    event,
                    "capture_session_id",
                )?)),
                finalized_meeting_ref: None,
            },
        }),
        "user_message" => Ok(AssistanceEvent::UserMessage {
            session_id,
            turn_id: ForegroundTurnId::new(payload_str(event, "turn_id")?),
            source_event_id: EvidenceId::new(payload_str(event, "source_event_id")?),
            text: payload_str(event, "text")?.to_string(),
        }),
        "role_changed" => Ok(AssistanceEvent::RoleCorrected {
            session_id,
            role: parse_role(payload_str(event, "role")?)?,
            source_event_id: EvidenceId::new(payload_str(event, "source_event_id")?),
        }),
        "posture_changed" => Ok(AssistanceEvent::PostureChanged {
            session_id,
            posture: parse_posture(payload_str(event, "posture")?)?,
        }),
        "speaker_corrected" => Ok(AssistanceEvent::SpeakerCorrected {
            session_id,
            source_label: payload_str(event, "from_speaker")?.to_string(),
            corrected_label: payload_str(event, "to_speaker")?.to_string(),
            source_event_id: EvidenceId::new(payload_str(event, "source_event_id")?),
        }),
        "background_started" => Ok(AssistanceEvent::BackgroundStarted {
            session_id,
            run_id: BackgroundRunId::new(payload_str(event, "run_id")?),
        }),
        "source_policy_invalidated" => Ok(AssistanceEvent::SourcePolicyInvalidated {
            session_id,
            new_generation: payload
                .get("source_policy_generation")
                .and_then(Value::as_u64)
                .ok_or_else(|| "missing source_policy_generation".to_string())?,
        }),
        "capture_stopped" => Ok(AssistanceEvent::CaptureStopped {
            session_id,
            capture_session_id: CaptureSessionId::new(payload_str(event, "capture_session_id")?),
        }),
        "processing_started" => Ok(AssistanceEvent::ProcessingStarted {
            session_id,
            capture_session_id: CaptureSessionId::new(payload_str(event, "capture_session_id")?),
        }),
        "meeting_finalized" => Ok(AssistanceEvent::MeetingFinalized {
            session_id,
            capture_session_id: CaptureSessionId::new(payload_str(event, "capture_session_id")?),
            meeting_ref: MeetingRef::new(payload_str(event, "meeting_ref")?),
        }),
        other => Err(format!("event kind {other} has no core reducer adapter")),
    }
}

fn reduction_summary(reduction: Reduction) -> Value {
    let action_invocations = reduction
        .actions
        .iter()
        .filter_map(|action| {
            let serialized = serde_json::to_value(action).expect("action serialization");
            serialized.get("invocation").map(|invocation| {
                json!({
                    "type": serialized.get("type").expect("tagged action type"),
                    "invocation": invocation,
                })
            })
        })
        .collect::<Vec<_>>();
    let action_types = reduction
        .actions
        .iter()
        .map(|action| {
            serde_json::to_value(action)
                .expect("action serialization")
                .get("type")
                .and_then(Value::as_str)
                .expect("tagged action type")
                .to_string()
        })
        .collect::<Vec<_>>();
    let mut summary = json!({
        "accepted": reduction.accepted,
        "action_types": action_types,
        "rejection": reduction.rejection,
    });
    if !action_invocations.is_empty() {
        summary
            .as_object_mut()
            .expect("reduction summary object")
            .insert(
                "action_invocations".to_string(),
                Value::Array(action_invocations),
            );
    }
    summary
}

fn session_summary(session: &LiveAssistanceSession) -> Value {
    let evidence: BTreeMap<&str, Value> = session
        .evidence
        .iter()
        .map(|(id, evidence)| {
            (
                id.as_str(),
                serde_json::to_value(evidence.source_kind).expect("evidence kind serialization"),
            )
        })
        .collect();
    let corrections: BTreeMap<&str, Value> = session
        .speaker_corrections
        .iter()
        .map(|(label, correction)| {
            (
                label.as_str(),
                json!({
                    "corrected_label": correction.corrected_label,
                    "revision": correction.revision,
                    "supersedes_revision": correction.supersedes_revision,
                    "source_event_id": correction.source_event_id.as_str(),
                }),
            )
        })
        .collect();
    json!({
        "scope": session.scope,
        "surface": session.surface,
        "phase": session.phase,
        "user_role": session.user_role,
        "posture": session.posture,
        "capture_mode": session.capture_mode,
        "capture_session_id": session.capture_session_id.as_ref().map(CaptureSessionId::as_str),
        "finalized_meeting_ref": session.finalized_meeting_ref.as_ref().map(MeetingRef::as_str),
        "source_policy_generation": session.source_policy_generation,
        "user_generation": session.user_generation,
        "foreground_turn_id": session.foreground_turn.as_ref().map(|turn| turn.id.as_str()),
        "background_run_id": session.background_run.as_ref().map(|run| run.id.as_str()),
        "evidence": evidence,
        "speaker_corrections": corrections,
    })
}

fn run_replay(fixture: &Value, replay: &Value) -> Result<Value, String> {
    let replay_name = required_str(replay, "name")?;
    let reducer_session_id = required_str(replay, "session_id")?;
    let surface = fixture
        .get("matrix")
        .and_then(|matrix| matrix.get("surfaces"))
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(Value::as_str)
        .ok_or_else(|| "fixture has no surface".to_string())?;
    let initial = fixture
        .get("initial_state")
        .ok_or_else(|| "fixture has no initial_state".to_string())?;
    let mut session = LiveAssistanceSession::new(
        LiveAssistanceSessionId::new(reducer_session_id),
        parse_surface(surface)?,
        parse_role(required_str(initial, "user_role")?)?,
        parse_posture(required_str(initial, "posture")?)?,
    );
    let events = fixture
        .get("events")
        .and_then(Value::as_array)
        .ok_or_else(|| "fixture has no events".to_string())?;
    let indexes = replay
        .get("event_indexes")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("replay {replay_name} has no event indexes"))?;
    let mut reductions = Vec::with_capacity(indexes.len());
    for raw_index in indexes {
        let index = raw_index
            .as_u64()
            .ok_or_else(|| format!("replay {replay_name} has non-integer event index"))?
            as usize;
        let event = events
            .get(index)
            .ok_or_else(|| format!("replay {replay_name} event index {index} is out of range"))?;
        let translated = translate_event(event)
            .map_err(|error| format!("replay {replay_name} event {index}: {error}"))?;
        reductions.push(reduction_summary(session.reduce(translated)));
    }
    Ok(json!({"reductions": reductions, "state": session_summary(&session)}))
}

fn fixture_paths() -> Vec<PathBuf> {
    let mut paths = fs::read_dir(fixtures_dir())
        .expect("read live-sidekick fixture directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

#[test]
fn executable_core_reducer_fixtures_are_expected_and_deterministic() {
    let mut executed = Map::new();
    for path in fixture_paths() {
        let fixture: Value = serde_json::from_slice(&fs::read(&path).expect("read fixture"))
            .expect("parse fixture JSON");
        let execution = fixture.get("execution").expect("execution metadata");
        if execution.get("target").and_then(Value::as_str) != Some("core_reducer") {
            continue;
        }
        let fixture_id = required_str(&fixture, "id").expect("fixture id");
        let replays = execution
            .get("replays")
            .and_then(Value::as_array)
            .expect("core reducer replays");
        let mut fixture_results = Map::new();
        for replay in replays {
            let replay_name = required_str(replay, "name").expect("replay name");
            let first = run_replay(&fixture, replay)
                .unwrap_or_else(|error| panic!("{fixture_id}: {error}"));
            let second = run_replay(&fixture, replay)
                .unwrap_or_else(|error| panic!("{fixture_id}: {error}"));
            assert_eq!(
                first, second,
                "{fixture_id}/{replay_name} was nondeterministic"
            );
            let expected = json!({
                "reductions": replay.get("expected_reductions").expect("expected reductions"),
                "state": replay.get("expected_state").expect("expected state"),
            });
            assert_eq!(first, expected, "{fixture_id}/{replay_name} contract drift");
            fixture_results.insert(replay_name.to_string(), first);
        }
        executed.insert(fixture_id.to_string(), Value::Object(fixture_results));
    }

    assert_eq!(executed.len(), 8, "executable reducer fixture count changed; update the schema contract and CI documentation intentionally");
    let replay_count: usize = executed
        .values()
        .map(|value| value.as_object().map_or(0, Map::len))
        .sum();
    assert_eq!(
        replay_count, 9,
        "deterministic reducer replay count changed unexpectedly"
    );
}
