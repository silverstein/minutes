#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VoiceRecoveryAction {
    RestartWorker,
    ReopenExact,
    ReplaceWithFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VoiceRecoveryStage {
    ExactRetryAvailable,
    FallbackRequired,
    FallbackExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MicrophoneDegradedReason {
    ExplicitSelection,
    DefaultUnchanged,
    ChangedDefaultAttachFailed,
    FallbackExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VoiceLineageFloor {
    pub(super) source_generation: u32,
    pub(super) discontinuity_epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct VoiceLowSignalNotice {
    pub(super) source_generation: u32,
    pub(super) discontinuity_epoch: u64,
    pub(super) peak_dbfs: Option<f64>,
    pub(super) rms_dbfs: Option<f64>,
}

#[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VoiceSourceHealth {
    AwaitingFirstFrame,
    Active,
    NoFrames,
    Stalled,
    ExactDigitalZeroPending,
    DigitallySilent,
    BelowThresholds,
    SustainedLowSignal,
    SignalObserved,
    NonFiniteSamples,
    SourceFailed,
}

#[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VoiceNativeFormat {
    pub(super) sample_rate_hz: u32,
    pub(super) channel_count: u16,
    pub(super) sample_representation: &'static str,
}

#[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct VoiceRecoveryContext {
    pub(super) health: VoiceSourceHealth,
    pub(super) native_format: Option<VoiceNativeFormat>,
    pub(super) frames_received_total: u64,
    pub(super) peak_dbfs: Option<f64>,
    pub(super) rms_dbfs: Option<f64>,
}
