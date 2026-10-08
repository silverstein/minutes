use super::voice_types::{
    MicrophoneDegradedReason, VoiceLowSignalNotice, VoiceRecoveryAction, VoiceRecoveryStage,
};
#[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
use super::voice_types::{VoiceRecoveryContext, VoiceSourceHealth};
use super::{cached_default_host, send_silence_notification_msg};

pub(super) fn recovery_action(
    stage: VoiceRecoveryStage,
    route_changed: bool,
    worker_failed: bool,
    automatic_fallback_allowed: bool,
) -> Option<VoiceRecoveryAction> {
    match stage {
        VoiceRecoveryStage::ExactRetryAvailable if worker_failed => {
            Some(VoiceRecoveryAction::RestartWorker)
        }
        VoiceRecoveryStage::ExactRetryAvailable if route_changed && automatic_fallback_allowed => {
            Some(VoiceRecoveryAction::ReplaceWithFallback)
        }
        VoiceRecoveryStage::ExactRetryAvailable => Some(VoiceRecoveryAction::ReopenExact),
        VoiceRecoveryStage::FallbackRequired if automatic_fallback_allowed => {
            Some(VoiceRecoveryAction::ReplaceWithFallback)
        }
        VoiceRecoveryStage::FallbackRequired | VoiceRecoveryStage::FallbackExhausted => None,
    }
}

pub(super) fn automatic_microphone_fallback_allowed(normalized_selection: Option<&str>) -> bool {
    normalized_selection.is_none_or(|selection| {
        let selection = selection.trim();
        selection.is_empty() || selection.eq_ignore_ascii_case("default")
    })
}

pub(super) fn default_microphone_changed(
    previous_device_id: Option<&str>,
    current_device_id: Option<&str>,
) -> bool {
    previous_device_id
        .zip(current_device_id)
        .is_some_and(|(previous, current)| previous != current)
}

pub(super) fn active_microphone_device_id<'a>(
    current_device_id: Option<&'a str>,
    initial_device_id: Option<&'a str>,
) -> Option<&'a str> {
    current_device_id.or(initial_device_id)
}

pub(super) fn current_default_microphone_device_id() -> Option<String> {
    use cpal::traits::{DeviceTrait, HostTrait};

    cached_default_host()
        .default_input_device()?
        .id()
        .ok()
        .map(|id| id.to_string())
}

#[cfg(all(feature = "pocketstation-capture", target_os = "macos"))]
pub(super) fn report_recovery_context(context: VoiceRecoveryContext) {
    eprintln!(
        "[minutes] PocketStation microphone recovery reason: {:?}",
        context.health
    );
    tracing::warn!(
        state = ?context.health,
        native_format = ?context.native_format,
        frames_received_total = context.frames_received_total,
        peak_dbfs = context.peak_dbfs,
        rms_dbfs = context.rms_dbfs,
        "PocketStation microphone needs host recovery"
    );
    let message = match context.health {
            VoiceSourceHealth::NoFrames => Some(
                "The selected microphone opened but delivered no audio frames. Minutes is retrying it while system audio continues.",
            ),
            VoiceSourceHealth::Stalled => Some(
                "The selected microphone stopped delivering audio. Minutes is retrying it while system audio continues.",
            ),
            VoiceSourceHealth::DigitallySilent => Some(
                "The selected microphone is delivering digital silence. Minutes is retrying it while system audio continues.",
            ),
            VoiceSourceHealth::NonFiniteSamples => Some(
                "The selected microphone delivered invalid samples. Minutes is retrying it while system audio continues.",
            ),
            VoiceSourceHealth::SourceFailed => Some(
                "The selected microphone failed. Minutes is retrying it while system audio continues.",
            ),
            _ => None,
    };
    if let Some(message) = message {
        send_silence_notification_msg(message);
    }
}

pub(super) fn report_sustained_low_signal(notice: VoiceLowSignalNotice) {
    let message = "The selected microphone is delivering sustained near-silence. Minutes will keep recording without switching microphones based on amplitude alone. Check the microphone mute/profile or choose another input if speech is missing.";
    eprintln!("[minutes] {message}");
    tracing::warn!(
        source_generation = notice.source_generation,
        discontinuity_epoch = notice.discontinuity_epoch,
        peak_dbfs = notice.peak_dbfs,
        rms_dbfs = notice.rms_dbfs,
        "PocketStation microphone sustained near-silence observation"
    );
    if let Err(error) = crate::logging::append_log(&serde_json::json!({
        "ts": chrono::Local::now().to_rfc3339(),
        "level": "warn",
        "step": "microphone_sustained_low_signal",
        "source_generation": notice.source_generation,
        "discontinuity_epoch": notice.discontinuity_epoch,
        "peak_dbfs": notice.peak_dbfs,
        "rms_dbfs": notice.rms_dbfs,
        "message": message,
    })) {
        tracing::warn!(%error, "failed to persist microphone low-signal diagnostic");
    }
    send_silence_notification_msg(message);
}

pub(super) fn should_report_low_signal(
    reported_continuity: &mut Option<(u32, u64)>,
    notice: VoiceLowSignalNotice,
) -> bool {
    let continuity = (notice.source_generation, notice.discontinuity_epoch);
    if *reported_continuity == Some(continuity) {
        return false;
    }
    *reported_continuity = Some(continuity);
    true
}

pub(super) fn microphone_degraded_message(reason: MicrophoneDegradedReason) -> &'static str {
    match reason {
        MicrophoneDegradedReason::ExplicitSelection => {
            "The selected microphone remained unusable after one exact retry. Minutes will not replace an explicitly selected microphone without your approval, so system audio will continue without microphone audio. Stop this recording, choose another microphone, then start a new recording."
        }
        MicrophoneDegradedReason::DefaultUnchanged => {
            "The microphone remained unusable after one exact retry, and the system default still resolves to the same physical input. Minutes will continue without microphone audio. Stop this recording, change the system default or choose another microphone, then start a new recording."
        }
        MicrophoneDegradedReason::ChangedDefaultAttachFailed => {
            "Minutes detected a changed system-default microphone, but attaching that input failed. System audio will continue without microphone audio. Stop this recording, correct or select the microphone, then start a new recording."
        }
        MicrophoneDegradedReason::FallbackExhausted => {
            "The microphone remained unusable after one exact retry and one changed-default attachment attempt. Minutes will continue without microphone audio. Stop this recording, correct or select the microphone, then start a new recording."
        }
    }
}

pub(super) fn continue_without_voice(
    independent_failure: bool,
    allow_degraded_call_capture: bool,
) -> bool {
    independent_failure || allow_degraded_call_capture
}
