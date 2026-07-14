use super::{CopilotModel, ModelHealth, ModelHealthStatus, ModelPrivacyClass};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutingPolicy {
    pub allow_cloud: bool,
    pub minimum_context_tokens: usize,
    pub target_latency_ms: u64,
}

impl RoutingPolicy {
    pub fn local_first(
        allow_cloud: bool,
        minimum_context_tokens: usize,
        target_latency_ms: u64,
    ) -> Self {
        Self {
            allow_cloud,
            minimum_context_tokens,
            target_latency_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderProbe {
    pub provider: String,
    pub model: String,
    pub health: ModelHealth,
    pub measured_latency_ms: u64,
    pub privacy_class: ModelPrivacyClass,
    pub context_window_tokens: usize,
    pub usable: bool,
    pub detail: String,
}

pub enum FastModelRoute {
    Selected {
        model: Arc<dyn CopilotModel>,
        probes: Vec<ProviderProbe>,
        detail: String,
    },
    SetupRequired {
        probes: Vec<ProviderProbe>,
        message: String,
    },
}

impl std::fmt::Debug for FastModelRoute {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Selected {
                model,
                probes,
                detail,
            } => formatter
                .debug_struct("Selected")
                .field("provider", &model.provider_name())
                .field("model", &model.model_name())
                .field("probes", probes)
                .field("detail", detail)
                .finish(),
            Self::SetupRequired { probes, message } => formatter
                .debug_struct("SetupRequired")
                .field("probes", probes)
                .field("message", message)
                .finish(),
        }
    }
}

/// Probe all eligible fast-lane models and choose from observed capability.
///
/// Availability comes from each provider's existing health contract, latency
/// is measured around that real probe plus prewarm, privacy is a hard cloud
/// gate and a soft local preference, and insufficient context is a hard gate.
/// No fixed provider ordering participates in the decision.
pub fn route_fast_model(
    candidates: Vec<Arc<dyn CopilotModel>>,
    requested_provider: Option<&str>,
    policy: RoutingPolicy,
) -> FastModelRoute {
    let requested_provider = requested_provider
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .filter(|provider| !provider.eq_ignore_ascii_case("auto-local"));
    let mut measured = Vec::<(Arc<dyn CopilotModel>, ProviderProbe, u128)>::new();

    for model in candidates {
        if requested_provider
            .is_some_and(|requested| !provider_matches(requested, model.provider_name()))
        {
            continue;
        }
        let privacy_class = model.privacy_class();
        let context_window_tokens = model.context_window_tokens();
        let started = Instant::now();
        let health = model.health();
        let health_available = health.status == ModelHealthStatus::Available;
        let privacy_allowed = policy.allow_cloud || privacy_class != ModelPrivacyClass::Cloud;
        let context_sufficient = context_window_tokens >= policy.minimum_context_tokens;

        let prewarm_error = if health_available && privacy_allowed && context_sufficient {
            model.prewarm().err()
        } else {
            None
        };
        let measured_latency_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let usable =
            health_available && privacy_allowed && context_sufficient && prewarm_error.is_none();
        let detail = if !privacy_allowed {
            "blocked by the local-only privacy setting".into()
        } else if !context_sufficient {
            format!(
                "context window is {context_window_tokens} tokens; at least {} are required",
                policy.minimum_context_tokens
            )
        } else if let Some(error) = prewarm_error {
            format!("provider probe succeeded but model prewarm failed: {error}")
        } else {
            health.detail.clone()
        };
        let probe = ProviderProbe {
            provider: model.provider_name().into(),
            model: model.model_name().into(),
            health,
            measured_latency_ms,
            privacy_class,
            context_window_tokens,
            usable,
            detail,
        };
        let score = route_score(&probe, policy);
        measured.push((model, probe, score));
    }

    let selected_index = measured
        .iter()
        .enumerate()
        .filter(|(_, (_, probe, _))| probe.usable)
        .min_by_key(|(_, (_, _, score))| *score)
        .map(|(index, _)| index);
    let probes = measured
        .iter()
        .map(|(_, probe, _)| probe.clone())
        .collect::<Vec<_>>();

    if let Some(index) = selected_index {
        let (model, probe, _) = &measured[index];
        let latency_note = if probe.measured_latency_ms <= policy.target_latency_ms {
            "within"
        } else {
            "above"
        };
        return FastModelRoute::Selected {
            model: Arc::clone(model),
            probes,
            detail: format!(
                "selected {} / {} after a {}ms live probe ({latency_note} the {}ms target, {:?}, {}-token context)",
                probe.provider,
                probe.model,
                probe.measured_latency_ms,
                policy.target_latency_ms,
                probe.privacy_class,
                probe.context_window_tokens,
            ),
        };
    }

    let requested_note = requested_provider
        .map(|provider| format!(" for the requested provider '{provider}'"))
        .unwrap_or_default();
    FastModelRoute::SetupRequired {
        probes,
        message: format!(
            "Copilot could not find a ready model{requested_note}. Your recording and transcript are unaffected, and no meeting content was sent. Start Ollama and install the configured model (for example `ollama pull llama3.2`), or choose another provider in Minutes, then start Copilot again."
        ),
    }
}

fn provider_matches(requested: &str, actual: &str) -> bool {
    requested.eq_ignore_ascii_case(actual)
        || (requested.eq_ignore_ascii_case("apple") && actual.eq_ignore_ascii_case("apple-fm"))
}

fn route_score(probe: &ProviderProbe, policy: RoutingPolicy) -> u128 {
    let privacy_penalty = match probe.privacy_class {
        ModelPrivacyClass::OnDevice => 0_u128,
        ModelPrivacyClass::LocalService => 25,
        ModelPrivacyClass::Cloud => 250,
    };
    let over_target_penalty = probe
        .measured_latency_ms
        .saturating_sub(policy.target_latency_ms) as u128;
    probe.measured_latency_ms as u128 + privacy_penalty + over_target_penalty
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copilot::{
        CancelToken, CopilotRequest, ModelError, ModelErrorKind, ModelEventSink, ModelStreamEvent,
        NudgeDraft, NudgeKind, OpportunityKind,
    };
    use chrono::Utc;
    use std::thread;
    use std::time::Duration;

    struct SimulatedModel {
        provider: &'static str,
        status: ModelHealthStatus,
        latency: Duration,
        privacy: ModelPrivacyClass,
        context: usize,
        prewarm_fails: bool,
    }

    impl CopilotModel for SimulatedModel {
        fn provider_name(&self) -> &str {
            self.provider
        }

        fn model_name(&self) -> &str {
            "simulated"
        }

        fn prewarm(&self) -> Result<(), ModelError> {
            if self.prewarm_fails {
                Err(ModelError::new(
                    ModelErrorKind::Unavailable,
                    "simulated prewarm failure",
                ))
            } else {
                Ok(())
            }
        }

        fn stream_structured(
            &self,
            _request: &CopilotRequest,
            _cancel: &CancelToken,
            sink: &dyn ModelEventSink,
        ) -> Result<NudgeDraft, ModelError> {
            let draft = NudgeDraft {
                kind: NudgeKind::Ask,
                text: "Ask".into(),
                source_chip: "test".into(),
                opportunity: OpportunityKind::General,
                confidence: 80,
            };
            sink.on_event(ModelStreamEvent::Structured(draft.clone()));
            Ok(draft)
        }

        fn health(&self) -> ModelHealth {
            thread::sleep(self.latency);
            ModelHealth {
                provider: self.provider.into(),
                model: "simulated".into(),
                status: self.status,
                detail: "simulated probe".into(),
                checked_ts: Utc::now(),
            }
        }

        fn privacy_class(&self) -> ModelPrivacyClass {
            self.privacy
        }

        fn context_window_tokens(&self) -> usize {
            self.context
        }
    }

    fn model(
        provider: &'static str,
        status: ModelHealthStatus,
        latency_ms: u64,
        privacy: ModelPrivacyClass,
        context: usize,
    ) -> Arc<dyn CopilotModel> {
        Arc::new(SimulatedModel {
            provider,
            status,
            latency: Duration::from_millis(latency_ms),
            privacy,
            context,
            prewarm_fails: false,
        })
    }

    fn policy(allow_cloud: bool) -> RoutingPolicy {
        RoutingPolicy::local_first(allow_cloud, 4_096, 100)
    }

    #[test]
    fn picks_a_healthy_provider_under_simulated_availability_and_latency() {
        let route = route_fast_model(
            vec![
                model(
                    "unavailable-local",
                    ModelHealthStatus::Unavailable,
                    1,
                    ModelPrivacyClass::LocalService,
                    8_192,
                ),
                model(
                    "slow-local",
                    ModelHealthStatus::Available,
                    35,
                    ModelPrivacyClass::LocalService,
                    8_192,
                ),
                model(
                    "fast-local",
                    ModelHealthStatus::Available,
                    2,
                    ModelPrivacyClass::LocalService,
                    8_192,
                ),
            ],
            None,
            policy(false),
        );
        let FastModelRoute::Selected { model, probes, .. } = route else {
            panic!("a healthy model should have been selected");
        };
        assert_eq!(model.provider_name(), "fast-local");
        assert_eq!(probes.len(), 3);
        assert!(!probes[0].usable);
    }

    #[test]
    fn context_capacity_and_privacy_gate_override_raw_speed() {
        let route = route_fast_model(
            vec![
                model(
                    "tiny",
                    ModelHealthStatus::Available,
                    1,
                    ModelPrivacyClass::OnDevice,
                    2_048,
                ),
                model(
                    "cloud-fast",
                    ModelHealthStatus::Available,
                    1,
                    ModelPrivacyClass::Cloud,
                    128_000,
                ),
                model(
                    "local-fit",
                    ModelHealthStatus::Available,
                    8,
                    ModelPrivacyClass::LocalService,
                    8_192,
                ),
            ],
            None,
            policy(false),
        );
        let FastModelRoute::Selected { model, probes, .. } = route else {
            panic!("the fitting local model should win");
        };
        assert_eq!(model.provider_name(), "local-fit");
        assert!(probes
            .iter()
            .any(|probe| probe.detail.contains("local-only")));
        assert!(probes
            .iter()
            .any(|probe| probe.detail.contains("context window")));
    }

    #[test]
    fn prewarm_failure_falls_through_to_a_working_provider() {
        let broken: Arc<dyn CopilotModel> = Arc::new(SimulatedModel {
            provider: "broken",
            status: ModelHealthStatus::Available,
            latency: Duration::ZERO,
            privacy: ModelPrivacyClass::OnDevice,
            context: 8_192,
            prewarm_fails: true,
        });
        let route = route_fast_model(
            vec![
                broken,
                model(
                    "working",
                    ModelHealthStatus::Available,
                    1,
                    ModelPrivacyClass::LocalService,
                    8_192,
                ),
            ],
            None,
            policy(false),
        );
        let FastModelRoute::Selected { model, probes, .. } = route else {
            panic!("routing should fall through");
        };
        assert_eq!(model.provider_name(), "working");
        assert!(probes[0].detail.contains("prewarm failed"));
    }

    #[test]
    fn no_usable_provider_returns_friendly_setup_state() {
        let route = route_fast_model(
            vec![model(
                "offline",
                ModelHealthStatus::Unavailable,
                0,
                ModelPrivacyClass::LocalService,
                8_192,
            )],
            None,
            policy(false),
        );
        let FastModelRoute::SetupRequired { message, .. } = route else {
            panic!("offline providers must produce setup state");
        };
        assert!(message.contains("recording and transcript are unaffected"));
        assert!(message.contains("ollama pull"));
    }
}
