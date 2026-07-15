use minutes_core::live_sidekick::{
    AssistanceAction, AssistanceEvent, AssistancePosture, AssistanceSurface, BackgroundRunId,
    CaptureMode, CaptureSessionId, EvidenceId, EvidenceSourceKind, ForegroundTurnId,
    InvocationIdentity, LiveAssistanceSession, LiveAssistanceSessionId, MeetingRef,
    ProviderAttestationId, ProviderBinding, ProviderBindingId, ProviderIsolationProfile, Reduction,
    UntrustedEvidence, UserRole,
};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
enum IssuedInvocation {
    Foreground {
        turn_id: ForegroundTurnId,
        invocation: InvocationIdentity,
    },
    Background {
        run_id: BackgroundRunId,
        invocation: InvocationIdentity,
    },
}

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

fn parse_provider_profile(value: &str) -> Result<ProviderIsolationProfile, String> {
    match value {
        "verified_loopback_text" => Ok(ProviderIsolationProfile::VerifiedLoopbackText),
        "agent_controlled_text" => Ok(ProviderIsolationProfile::AgentControlledText),
        "agent_controlled_exact_session_screen" => {
            Ok(ProviderIsolationProfile::AgentControlledExactSessionScreen)
        }
        "unavailable" => Ok(ProviderIsolationProfile::Unavailable),
        other => Err(format!("unsupported provider isolation profile {other}")),
    }
}

fn invocation_source_index(event: &Value) -> Result<usize, String> {
    event
        .get("payload")
        .and_then(|payload| payload.get("invocation_from_event_index"))
        .and_then(Value::as_u64)
        .ok_or_else(|| "missing invocation_from_event_index".to_string())
        .and_then(|value| {
            usize::try_from(value)
                .map_err(|_| "invocation_from_event_index exceeds platform usize".to_string())
        })
}

fn translate_event(
    event: &Value,
    issued: &BTreeMap<usize, IssuedInvocation>,
) -> Result<AssistanceEvent, String> {
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
        "foreground_completed" => {
            let turn_id = ForegroundTurnId::new(payload_str(event, "turn_id")?);
            let source_index = invocation_source_index(event)?;
            let invocation = match issued.get(&source_index) {
                Some(IssuedInvocation::Foreground {
                    turn_id: issued_turn,
                    invocation,
                }) if issued_turn == &turn_id => *invocation,
                Some(_) => return Err("foreground invocation reference did not match turn".into()),
                None => return Err("foreground invocation reference was not issued".into()),
            };
            Ok(AssistanceEvent::ForegroundCompleted {
                session_id,
                turn_id,
                invocation,
            })
        }
        "background_completed" => {
            let run_id = BackgroundRunId::new(payload_str(event, "run_id")?);
            let source_index = invocation_source_index(event)?;
            let invocation = match issued.get(&source_index) {
                Some(IssuedInvocation::Background {
                    run_id: issued_run,
                    invocation,
                }) if issued_run == &run_id => *invocation,
                Some(_) => return Err("background invocation reference did not match run".into()),
                None => return Err("background invocation reference was not issued".into()),
            };
            Ok(AssistanceEvent::BackgroundCompleted {
                session_id,
                run_id,
                invocation,
            })
        }
        "provider_binding_changed" => {
            let generation = payload
                .get("binding_generation")
                .and_then(Value::as_u64)
                .ok_or_else(|| "missing binding_generation".to_string())?;
            let binding = ProviderBinding::new(
                ProviderBindingId::new(payload_str(event, "binding_id")?),
                generation,
                ProviderAttestationId::new(payload_str(event, "attestation_id")?),
                parse_provider_profile(payload_str(event, "isolation_profile")?)?,
            )
            .ok_or_else(|| "invalid provider binding".to_string())?;
            Ok(AssistanceEvent::ProviderBindingChanged {
                session_id,
                binding,
            })
        }
        "meeting_artifact_observed" | "repository_result_observed" => {
            let source_kind = if kind == "meeting_artifact_observed" {
                EvidenceSourceKind::MeetingArtifact
            } else {
                EvidenceSourceKind::RepositoryResult
            };
            Ok(AssistanceEvent::EvidenceObserved {
                session_id,
                evidence: UntrustedEvidence {
                    id: EvidenceId::new(payload_str(event, "event_id")?),
                    source_kind,
                    capture_session_id: None,
                    finalized_meeting_ref: Some(MeetingRef::new(payload_str(
                        event,
                        "finalized_meeting_ref",
                    )?)),
                },
            })
        }
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
    serde_json::to_value(reduction).expect("reduction serialization")
}

fn remember_invocation(
    event_index: usize,
    reduction: &Reduction,
    issued: &mut BTreeMap<usize, IssuedInvocation>,
) {
    for action in &reduction.actions {
        match action {
            AssistanceAction::RequestReadOnlyForegroundInference {
                turn_id,
                invocation,
                ..
            } => {
                issued.insert(
                    event_index,
                    IssuedInvocation::Foreground {
                        turn_id: turn_id.clone(),
                        invocation: *invocation,
                    },
                );
            }
            AssistanceAction::BackgroundInvocationRegistered { run_id, invocation } => {
                issued.insert(
                    event_index,
                    IssuedInvocation::Background {
                        run_id: run_id.clone(),
                        invocation: *invocation,
                    },
                );
            }
            _ => {}
        }
    }
}

fn session_summary(session: &LiveAssistanceSession) -> Value {
    let evidence: BTreeMap<&str, Value> = session
        .evidence
        .iter()
        .map(|(id, evidence)| {
            (
                id.as_str(),
                serde_json::to_value(evidence).expect("evidence serialization"),
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
    let mut summary = json!({
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
        "provider_capabilities": session.provider_binding.as_ref().map(ProviderBinding::capabilities),
        "foreground_turn": session.foreground_turn,
        "background_run": session.background_run,
        "evidence": evidence,
        "speaker_corrections": corrections,
    });
    if let Some(binding) = session.provider_binding.as_ref() {
        summary
            .as_object_mut()
            .expect("session summary object")
            .insert(
                "provider_binding".into(),
                serde_json::to_value(binding).expect("provider binding serialization"),
            );
    }
    summary
}

fn run_replay(fixture: &Value, replay: &Value) -> Result<Value, String> {
    let replay_name = required_str(replay, "name")?;
    let reducer_session_id = required_str(replay, "session_id")?;
    let surface = required_str(replay, "surface")?;
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
    let mut issued = BTreeMap::new();
    for raw_index in indexes {
        let index = raw_index
            .as_u64()
            .ok_or_else(|| format!("replay {replay_name} has non-integer event index"))?
            as usize;
        let event = events
            .get(index)
            .ok_or_else(|| format!("replay {replay_name} event index {index} is out of range"))?;
        let translated = translate_event(event, &issued)
            .map_err(|error| format!("replay {replay_name} event {index}: {error}"))?;
        let reduction = session.reduce(translated);
        remember_invocation(index, &reduction, &mut issued);
        reductions.push(reduction_summary(reduction));
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

    assert_eq!(executed.len(), 13, "executable reducer fixture count changed; update the schema contract and CI documentation intentionally");
    let replay_count: usize = executed
        .values()
        .map(|value| value.as_object().map_or(0, Map::len))
        .sum();
    assert_eq!(
        replay_count, 16,
        "deterministic reducer replay count changed unexpectedly"
    );
}

#[test]
fn completion_adapters_require_prior_matching_reducer_identity() {
    let foreground = json!({
        "kind": "foreground_completed",
        "session_id": "SESSION_A",
        "payload": {
            "turn_id": "FOREGROUND_A",
            "invocation_from_event_index": 1
        }
    });
    assert_eq!(
        translate_event(&foreground, &BTreeMap::new()).unwrap_err(),
        "foreground invocation reference was not issued"
    );

    let invocation = InvocationIdentity {
        sequence: 7,
        source_policy_generation: 2,
        user_generation: 3,
    };
    let mut issued = BTreeMap::new();
    issued.insert(
        1,
        IssuedInvocation::Background {
            run_id: BackgroundRunId::new("BACKGROUND_A"),
            invocation,
        },
    );
    assert_eq!(
        translate_event(&foreground, &issued).unwrap_err(),
        "foreground invocation reference did not match turn"
    );

    issued.insert(
        1,
        IssuedInvocation::Foreground {
            turn_id: ForegroundTurnId::new("FOREGROUND_A"),
            invocation,
        },
    );
    assert!(matches!(
        translate_event(&foreground, &issued),
        Ok(AssistanceEvent::ForegroundCompleted {
            invocation: actual,
            ..
        }) if actual == invocation
    ));

    let background = json!({
        "kind": "background_completed",
        "session_id": "SESSION_A",
        "payload": {
            "run_id": "BACKGROUND_A",
            "invocation_from_event_index": 1
        }
    });
    assert_eq!(
        translate_event(&background, &issued).unwrap_err(),
        "background invocation reference did not match run"
    );
}

#[test]
fn provider_binding_adapter_requires_identity_generation_and_attestation() {
    let event = json!({
        "kind": "provider_binding_changed",
        "session_id": "SESSION_A",
        "payload": {
            "binding_id": "ROUTE_A",
            "binding_generation": 4,
            "attestation_id": "ATTESTATION_A",
            "isolation_profile": "agent_controlled_exact_session_screen"
        }
    });
    for field in [
        "binding_id",
        "binding_generation",
        "attestation_id",
        "isolation_profile",
    ] {
        let mut incomplete = event.clone();
        incomplete["payload"]
            .as_object_mut()
            .expect("payload object")
            .remove(field);
        assert!(
            translate_event(&incomplete, &BTreeMap::new()).is_err(),
            "provider adapter accepted a binding without {field}"
        );
    }

    assert!(matches!(
        translate_event(&event, &BTreeMap::new()),
        Ok(AssistanceEvent::ProviderBindingChanged { binding, .. })
            if binding.binding_id().as_str() == "ROUTE_A"
                && binding.generation() == 4
                && binding.attestation_id().as_str() == "ATTESTATION_A"
                && binding.profile() == ProviderIsolationProfile::AgentControlledExactSessionScreen
    ));
}
