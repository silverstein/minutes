use super::health::MicrophoneObservations;
use super::selection::{MicrophoneDiagnosticIdentity, MicrophoneSelection};
use super::{capture_error, PocketStationMicrophoneStream};
use crate::error::CaptureError;
use pocketstation::{DeviceSelector, SessionSourceReplacement, SessionSourceReplacementError};
use std::time::{Duration, Instant};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(3);
const PENDING_REPLACEMENT_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) enum MicrophoneControl {
    Reopen {
        selector: DeviceSelector,
        identity: MicrophoneDiagnosticIdentity,
        response: crossbeam_channel::Sender<MicrophoneControlResult>,
    },
    Replace {
        selector: DeviceSelector,
        identity: MicrophoneDiagnosticIdentity,
        response: crossbeam_channel::Sender<MicrophoneControlResult>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MicrophoneReplacementOutcome {
    Completed(SessionSourceReplacement),
    Pending {
        source_generation: u32,
        discontinuity_epoch: u64,
        timeout_ms: u64,
    },
}

impl MicrophoneReplacementOutcome {
    pub(crate) fn continuity(self) -> (u32, u64) {
        match self {
            Self::Completed(replacement) => (
                replacement.source_generation,
                replacement.discontinuity_epoch,
            ),
            Self::Pending {
                source_generation,
                discontinuity_epoch,
                ..
            } => (source_generation, discontinuity_epoch),
        }
    }

    pub(crate) const fn is_pending(self) -> bool {
        matches!(self, Self::Pending { .. })
    }
}

pub(super) type MicrophoneControlResult =
    Result<SessionSourceReplacement, SessionSourceReplacementError>;

pub(super) struct PendingMicrophoneReplacement {
    source_generation: u32,
    discontinuity_epoch: u64,
    selection: Option<MicrophoneSelection>,
    late_response: Option<crossbeam_channel::Receiver<MicrophoneControlResult>>,
    started_at: Instant,
}

struct MicrophoneControlRequest {
    outcome: MicrophoneReplacementOutcome,
    late_response: Option<crossbeam_channel::Receiver<MicrophoneControlResult>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconciledMicrophoneReplacement {
    pub(crate) device_name: String,
    pub(crate) source_generation: u32,
    pub(crate) discontinuity_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MicrophoneReplacementReconciliation {
    Attached(ReconciledMicrophoneReplacement),
    Detached {
        source_generation: u32,
        discontinuity_epoch: u64,
    },
    Failed(String),
}

pub(super) fn replacement_continuity_reached(
    observations: MicrophoneObservations,
    source_generation: u32,
    discontinuity_epoch: u64,
) -> bool {
    observations.replacement.is_some_and(|replacement| {
        replacement.attached_source_id.is_some()
            && (
                replacement.source_generation,
                replacement.discontinuity_epoch,
            ) == (source_generation, discontinuity_epoch)
    })
}

impl PocketStationMicrophoneStream {
    pub(crate) fn replacement_pending(&self) -> bool {
        self.pending_replacement.is_some()
    }

    pub(crate) fn reconcile_pending_replacement(
        &mut self,
    ) -> Option<MicrophoneReplacementReconciliation> {
        let (source_generation, discontinuity_epoch) = self
            .pending_replacement
            .as_ref()
            .map(|pending| (pending.source_generation, pending.discontinuity_epoch))?;
        let late_response = self
            .pending_replacement
            .as_mut()
            .and_then(|pending| pending.late_response.take());
        if let Some(receiver) = late_response {
            match receiver.try_recv() {
                Ok(Ok(replacement)) => {
                    if (
                        replacement.source_generation,
                        replacement.discontinuity_epoch,
                    ) != (source_generation, discontinuity_epoch)
                    {
                        return Some(self.fail_pending_replacement(format!(
                            "microphone replacement returned unexpected continuity ({}, {}) instead of ({source_generation}, {discontinuity_epoch})",
                            replacement.source_generation, replacement.discontinuity_epoch
                        )));
                    }
                    return Some(self.attach_pending_replacement(
                        replacement.source_generation,
                        replacement.discontinuity_epoch,
                    ));
                }
                Ok(Err(SessionSourceReplacementError::ResponseTimedOut { .. })) => {}
                Ok(Err(error)) => {
                    return Some(self.fail_pending_replacement(format!(
                        "microphone replacement failed after the caller timeout: {error}"
                    )));
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    if let Some(pending) = self.pending_replacement.as_mut() {
                        pending.late_response = Some(receiver);
                    }
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    return Some(
                        self.fail_pending_replacement(
                            "microphone replacement worker disconnected before reporting a result"
                                .to_owned(),
                        ),
                    );
                }
            }
        }

        let observations = self.observations();
        let replacement = match observations.replacement {
            Some(replacement) => replacement,
            None if self.pending_replacement.as_ref().is_some_and(|pending| {
                pending.started_at.elapsed() >= PENDING_REPLACEMENT_TIMEOUT
            }) =>
            {
                return Some(self.fail_pending_replacement(format!(
                    "microphone replacement produced no observations inside {} ms",
                    PENDING_REPLACEMENT_TIMEOUT.as_millis()
                )));
            }
            None => return None,
        };
        let observed_continuity = (
            replacement.source_generation,
            replacement.discontinuity_epoch,
        );
        let target_continuity = (source_generation, discontinuity_epoch);
        if observed_continuity < target_continuity {
            if self
                .pending_replacement
                .as_ref()
                .is_some_and(|pending| pending.started_at.elapsed() >= PENDING_REPLACEMENT_TIMEOUT)
            {
                return Some(self.fail_pending_replacement(format!(
                    "microphone replacement remained indeterminate for {} ms",
                    PENDING_REPLACEMENT_TIMEOUT.as_millis()
                )));
            }
            return None;
        }
        if observed_continuity > target_continuity {
            return Some(self.fail_pending_replacement(format!(
                "microphone replacement observations advanced to unexpected continuity ({}, {})",
                replacement.source_generation, replacement.discontinuity_epoch
            )));
        }
        if replacement.attached_source_id.is_none() {
            if self
                .pending_replacement
                .as_ref()
                .is_some_and(|pending| pending.started_at.elapsed() < PENDING_REPLACEMENT_TIMEOUT)
            {
                return None;
            }
            self.clear_pending_replacement();
            self.worker.mark_failed();
            return Some(MicrophoneReplacementReconciliation::Detached {
                source_generation: replacement.source_generation,
                discontinuity_epoch: replacement.discontinuity_epoch,
            });
        }
        Some(self.attach_pending_replacement(
            observations.source_generation,
            observations.discontinuity_epoch,
        ))
    }

    fn attach_pending_replacement(
        &mut self,
        source_generation: u32,
        discontinuity_epoch: u64,
    ) -> MicrophoneReplacementReconciliation {
        if let Some(selection) = self
            .pending_replacement
            .take()
            .and_then(|pending| pending.selection)
        {
            self.selector = selection.selector;
            self.device_id = selection.device_id;
            self.device_name = selection.display_name;
        }
        MicrophoneReplacementReconciliation::Attached(ReconciledMicrophoneReplacement {
            device_name: self.device_name.clone(),
            source_generation,
            discontinuity_epoch,
        })
    }

    fn fail_pending_replacement(&mut self, message: String) -> MicrophoneReplacementReconciliation {
        self.clear_pending_replacement();
        self.worker.mark_failed();
        MicrophoneReplacementReconciliation::Failed(message)
    }

    fn clear_pending_replacement(&mut self) {
        self.pending_replacement = None;
    }

    pub(crate) fn reopen_exact(&mut self) -> Result<MicrophoneReplacementOutcome, CaptureError> {
        let selector = self.selector.clone();
        let identity = MicrophoneDiagnosticIdentity {
            device_id: self.device_id.clone(),
            display_name: self.device_name.clone(),
        };
        let request = self.request_control(|response| MicrophoneControl::Reopen {
            selector,
            identity,
            response,
        })?;
        let outcome = request.outcome;
        if let MicrophoneReplacementOutcome::Pending {
            source_generation,
            discontinuity_epoch,
            ..
        } = outcome
        {
            self.pending_replacement = Some(Box::new(PendingMicrophoneReplacement {
                source_generation,
                discontinuity_epoch,
                selection: None,
                late_response: request.late_response,
                started_at: Instant::now(),
            }));
        }
        Ok(outcome)
    }

    pub(crate) fn replace_with_fallback(
        &mut self,
        resolved_default_name: String,
    ) -> Result<MicrophoneReplacementOutcome, CaptureError> {
        let selection =
            MicrophoneSelection::recovery_fallback(Some(&self.device_id), &resolved_default_name)?;
        let selector = selection.selector.clone();
        let identity = MicrophoneDiagnosticIdentity::from(&selection);
        let request = self.request_control(|response| MicrophoneControl::Replace {
            selector: selector.clone(),
            identity,
            response,
        })?;
        let outcome = request.outcome;
        match outcome {
            MicrophoneReplacementOutcome::Completed(_) => {
                self.selector = selector;
                self.device_id = selection.device_id;
                self.device_name = selection.display_name;
                self.pending_replacement = None;
            }
            MicrophoneReplacementOutcome::Pending {
                source_generation,
                discontinuity_epoch,
                ..
            } => {
                self.pending_replacement = Some(Box::new(PendingMicrophoneReplacement {
                    source_generation,
                    discontinuity_epoch,
                    selection: Some(selection),
                    late_response: request.late_response,
                    started_at: Instant::now(),
                }));
            }
        }
        Ok(outcome)
    }

    fn request_control(
        &self,
        command: impl FnOnce(crossbeam_channel::Sender<MicrophoneControlResult>) -> MicrophoneControl,
    ) -> Result<MicrophoneControlRequest, CaptureError> {
        if self.pending_replacement.is_some() {
            return Err(capture_error(
                "apply PocketStation microphone control",
                "a prior microphone replacement is still pending",
            ));
        }
        let observations = self.observations();
        let pending_generation = observations.source_generation.saturating_add(1);
        let pending_discontinuity = observations.discontinuity_epoch.saturating_add(1);
        let (response, receiver) = crossbeam_channel::bounded(1);
        self.worker
            .control()
            .send_timeout(command(response), CONTROL_TIMEOUT)
            .map_err(|error| capture_error("send PocketStation microphone control", error))?;
        match receiver.recv_timeout(CONTROL_TIMEOUT) {
            Ok(Ok(replacement)) => Ok(MicrophoneControlRequest {
                outcome: MicrophoneReplacementOutcome::Completed(replacement),
                late_response: None,
            }),
            Ok(Err(SessionSourceReplacementError::ResponseTimedOut { timeout_ms })) => {
                Ok(MicrophoneControlRequest {
                    outcome: MicrophoneReplacementOutcome::Pending {
                        source_generation: pending_generation,
                        discontinuity_epoch: pending_discontinuity,
                        timeout_ms,
                    },
                    late_response: None,
                })
            }
            Ok(Err(error)) => Err(capture_error(
                "apply PocketStation microphone control",
                error,
            )),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => Ok(MicrophoneControlRequest {
                outcome: MicrophoneReplacementOutcome::Pending {
                    source_generation: pending_generation,
                    discontinuity_epoch: pending_discontinuity,
                    timeout_ms: CONTROL_TIMEOUT.as_millis() as u64,
                },
                late_response: Some(receiver),
            }),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Err(capture_error(
                "wait for PocketStation microphone control",
                "microphone worker stopped before reporting the result",
            )),
        }
    }
}
