use super::voice_types::{VoiceLineageFloor, VoiceLowSignalNotice, VoiceRecoveryAction};
#[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
use super::voice_types::{VoiceRecoveryContext, VoiceSourceHealth};
use super::DualCapturePlan;
#[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
use super::{cached_default_host, select_device_with_override};
use crate::error::CaptureError;
use crate::streaming::{AudioChunk, AudioStream};

/// Adapts the active microphone implementation to the facts the recording
/// loop needs. Recovery choices and user-facing policy stay in
/// `voice_recovery`; this module only starts, observes, and replaces sources.
pub(super) enum VoiceCaptureStream {
    Cpal(AudioStream),
    #[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
    PocketStation(crate::pocketstation_microphone::PocketStationMicrophoneStream),
}

pub(super) struct RecoveredVoiceStream {
    pub(super) stream: VoiceCaptureStream,
    pub(super) lineage_floor: Option<VoiceLineageFloor>,
    pub(super) replacement_pending: bool,
}

#[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
pub(super) enum VoiceReplacementReconciliation {
    Attached {
        previous_device_name: String,
        device_name: String,
        lineage_floor: VoiceLineageFloor,
    },
    Detached {
        source_generation: u32,
        discontinuity_epoch: u64,
    },
    Failed(String),
}

impl VoiceCaptureStream {
    pub(super) fn start(plan: &DualCapturePlan) -> Result<Self, CaptureError> {
        #[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
        if plan.call_capture_backend
            == crate::system_audio_backend::CaptureBackendKind::CoreAudioTap
        {
            return crate::pocketstation_microphone::PocketStationMicrophoneStream::start(
                plan.voice_override.as_deref(),
                &plan.voice_device_name,
            )
            .map(Self::PocketStation);
        }

        AudioStream::start(plan.voice_override.as_deref()).map(Self::Cpal)
    }

    pub(super) fn start_fallback(plan: &DualCapturePlan) -> Result<Self, CaptureError> {
        #[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
        if plan.uses_pocketstation_microphone() {
            return crate::pocketstation_microphone::PocketStationMicrophoneStream::start_fallback(
                plan.voice_device_id.as_deref(),
                &plan.voice_device_name,
            )
            .map(Self::PocketStation);
        }

        Self::start(plan)
    }

    pub(super) fn receiver(&self) -> crossbeam_channel::Receiver<AudioChunk> {
        match self {
            Self::Cpal(stream) => stream.receiver.clone(),
            #[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
            Self::PocketStation(stream) => stream.receiver(),
        }
    }

    pub(super) fn device_name(&self) -> &str {
        match self {
            Self::Cpal(stream) => &stream.device_name,
            #[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
            Self::PocketStation(stream) => stream.device_name(),
        }
    }

    pub(super) fn device_id(&self) -> Option<&str> {
        match self {
            Self::Cpal(_) => None,
            #[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
            Self::PocketStation(stream) => Some(stream.device_id()),
        }
    }

    pub(super) fn replacement_pending(&self) -> bool {
        match self {
            Self::Cpal(_) => false,
            #[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
            Self::PocketStation(stream) => stream.replacement_pending(),
        }
    }

    #[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
    pub(super) fn reconcile_pending_replacement(
        &mut self,
    ) -> Option<VoiceReplacementReconciliation> {
        use crate::pocketstation_microphone::MicrophoneReplacementReconciliation as Reconciliation;

        let previous_device_name = self.device_name().to_owned();
        match self {
            Self::Cpal(_) => None,
            Self::PocketStation(stream) => {
                stream
                    .reconcile_pending_replacement()
                    .map(|reconciliation| match reconciliation {
                        Reconciliation::Attached(completion) => {
                            VoiceReplacementReconciliation::Attached {
                                previous_device_name,
                                device_name: completion.device_name,
                                lineage_floor: VoiceLineageFloor {
                                    source_generation: completion.source_generation,
                                    discontinuity_epoch: completion.discontinuity_epoch,
                                },
                            }
                        }
                        Reconciliation::Detached {
                            source_generation,
                            discontinuity_epoch,
                        } => VoiceReplacementReconciliation::Detached {
                            source_generation,
                            discontinuity_epoch,
                        },
                        Reconciliation::Failed(message) => {
                            VoiceReplacementReconciliation::Failed(message)
                        }
                    })
            }
        }
    }

    pub(super) fn needs_recovery(&self) -> bool {
        match self {
            Self::Cpal(stream) => stream.has_error(),
            #[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
            Self::PocketStation(stream) => {
                let observations = stream.observations();
                stream.has_error() || observations.requires_recovery()
            }
        }
    }

    pub(super) fn worker_failed(&self) -> bool {
        match self {
            Self::Cpal(stream) => stream.has_error(),
            #[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
            Self::PocketStation(stream) => stream.has_error(),
        }
    }

    pub(super) fn recovery_confirmed(&self) -> bool {
        match self {
            Self::Cpal(stream) => !stream.has_error(),
            #[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
            Self::PocketStation(stream) => stream.observations().confirms_recovery(),
        }
    }

    pub(super) fn low_signal_notice(&self) -> Option<VoiceLowSignalNotice> {
        match self {
            Self::Cpal(_) => None,
            #[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
            Self::PocketStation(stream) => {
                use crate::pocketstation_microphone::MicrophoneSignalState;
                let observations = stream.observations();
                (observations.state == MicrophoneSignalState::SustainedLowSignal).then(|| {
                    VoiceLowSignalNotice {
                        source_generation: observations.source_generation,
                        discontinuity_epoch: observations.discontinuity_epoch,
                        peak_dbfs: observations
                            .signal
                            .and_then(|signal| signal.window_peak_dbfs()),
                        rms_dbfs: observations
                            .signal
                            .and_then(|signal| signal.window_rms_dbfs()),
                    }
                })
            }
        }
    }

    #[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
    pub(super) fn recovery_context(&self) -> Option<VoiceRecoveryContext> {
        match self {
            Self::Cpal(_) => None,
            Self::PocketStation(stream) => {
                use crate::pocketstation_microphone::MicrophoneSignalState;

                let observations = stream.observations();
                let health = match observations.state {
                    MicrophoneSignalState::AwaitingFirstFrame => {
                        VoiceSourceHealth::AwaitingFirstFrame
                    }
                    MicrophoneSignalState::Active => VoiceSourceHealth::Active,
                    MicrophoneSignalState::NoFrames => VoiceSourceHealth::NoFrames,
                    MicrophoneSignalState::Stalled => VoiceSourceHealth::Stalled,
                    MicrophoneSignalState::ExactDigitalZeroPending => {
                        VoiceSourceHealth::ExactDigitalZeroPending
                    }
                    MicrophoneSignalState::DigitallySilent => VoiceSourceHealth::DigitallySilent,
                    MicrophoneSignalState::BelowThresholds => VoiceSourceHealth::BelowThresholds,
                    MicrophoneSignalState::SustainedLowSignal => {
                        VoiceSourceHealth::SustainedLowSignal
                    }
                    MicrophoneSignalState::SignalObserved => VoiceSourceHealth::SignalObserved,
                    MicrophoneSignalState::NonFiniteSamples => VoiceSourceHealth::NonFiniteSamples,
                    MicrophoneSignalState::SourceFailed => VoiceSourceHealth::SourceFailed,
                };
                Some(VoiceRecoveryContext {
                    health,
                    native_format: observations.native_format.map(|format| {
                        use pocketstation::CaptureSampleRepresentation as Representation;

                        let sample_representation = match format.sample_representation {
                            Representation::SignedInteger8 => "signed_integer_8",
                            Representation::SignedInteger16 => "signed_integer_16",
                            Representation::SignedInteger24 => "signed_integer_24",
                            Representation::SignedInteger32 => "signed_integer_32",
                            Representation::SignedInteger64 => "signed_integer_64",
                            Representation::UnsignedInteger8 => "unsigned_integer_8",
                            Representation::UnsignedInteger16 => "unsigned_integer_16",
                            Representation::UnsignedInteger24 => "unsigned_integer_24",
                            Representation::UnsignedInteger32 => "unsigned_integer_32",
                            Representation::UnsignedInteger64 => "unsigned_integer_64",
                            Representation::Float32 => "float_32",
                            Representation::Float64 => "float_64",
                        };
                        super::voice_types::VoiceNativeFormat {
                            sample_rate_hz: format.sample_rate_hz,
                            channel_count: format.channel_count,
                            sample_representation,
                        }
                    }),
                    frames_received_total: observations
                        .activity
                        .map_or(0, |activity| activity.frames_received_total),
                    peak_dbfs: observations
                        .signal
                        .and_then(|signal| signal.window_peak_dbfs()),
                    rms_dbfs: observations
                        .signal
                        .and_then(|signal| signal.window_rms_dbfs()),
                })
            }
        }
    }

    pub(super) fn independent_failure(&self) -> bool {
        match self {
            Self::Cpal(_) => false,
            #[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
            Self::PocketStation(_) => true,
        }
    }

    pub(super) fn delivering_frames(&self) -> bool {
        match self {
            Self::Cpal(_) => true,
            #[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
            Self::PocketStation(stream) => {
                use crate::pocketstation_microphone::MicrophoneSignalState;
                !matches!(
                    stream.observations().state,
                    MicrophoneSignalState::NoFrames
                        | MicrophoneSignalState::Stalled
                        | MicrophoneSignalState::SourceFailed
                )
            }
        }
    }

    pub(super) fn recover(
        self,
        plan: &DualCapturePlan,
        action: VoiceRecoveryAction,
    ) -> Result<RecoveredVoiceStream, CaptureError> {
        #[cfg(not(all(feature = "pocketstation-capture", target_os = "macos")))]
        let _ = action;

        match self {
            Self::Cpal(stream) => {
                drop(stream);
                AudioStream::start(plan.voice_override.as_deref()).map(|stream| {
                    RecoveredVoiceStream {
                        stream: Self::Cpal(stream),
                        lineage_floor: None,
                        replacement_pending: false,
                    }
                })
            }
            #[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
            Self::PocketStation(mut stream) => {
                if action == VoiceRecoveryAction::RestartWorker || stream.has_error() {
                    let previous_device_id = stream.device_id().to_owned();
                    drop(stream);
                    let restarted = match action {
                        VoiceRecoveryAction::ReplaceWithFallback => {
                            crate::pocketstation_microphone::PocketStationMicrophoneStream::start_fallback(
                                Some(&previous_device_id),
                                &plan.voice_device_name,
                            )
                        }
                        VoiceRecoveryAction::RestartWorker | VoiceRecoveryAction::ReopenExact => {
                            crate::pocketstation_microphone::PocketStationMicrophoneStream::start(
                                plan.voice_override.as_deref(),
                                &plan.voice_device_name,
                            )
                        }
                    };
                    return restarted.map(|stream| RecoveredVoiceStream {
                        stream: Self::PocketStation(stream),
                        lineage_floor: None,
                        replacement_pending: false,
                    });
                }

                let outcome = match action {
                    VoiceRecoveryAction::RestartWorker => unreachable!("handled above"),
                    VoiceRecoveryAction::ReopenExact => stream.reopen_exact(),
                    VoiceRecoveryAction::ReplaceWithFallback => {
                        let (_, default_name) =
                            select_device_with_override(cached_default_host(), None)?;
                        stream.replace_with_fallback(default_name)
                    }
                }?;
                let (source_generation, discontinuity_epoch) = outcome.continuity();
                let replacement_pending = outcome.is_pending();
                tracing::info!(
                    source_generation,
                    discontinuity_epoch,
                    replacement_pending,
                    action = ?action,
                    "PocketStation microphone recovery requested a new source generation"
                );
                Ok(RecoveredVoiceStream {
                    stream: Self::PocketStation(stream),
                    lineage_floor: recovery_lineage_floor(
                        action,
                        replacement_pending,
                        source_generation,
                        discontinuity_epoch,
                    ),
                    replacement_pending,
                })
            }
        }
    }
}

#[cfg(any(test, all(feature = "pocketstation-capture", target_os = "macos")))]
pub(super) fn recovery_lineage_floor(
    action: VoiceRecoveryAction,
    replacement_pending: bool,
    source_generation: u32,
    discontinuity_epoch: u64,
) -> Option<VoiceLineageFloor> {
    (!replacement_pending || action == VoiceRecoveryAction::ReopenExact).then_some(
        VoiceLineageFloor {
            source_generation,
            discontinuity_epoch,
        },
    )
}
