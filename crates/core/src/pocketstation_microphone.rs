mod chunks;
mod health;
mod recovery;
mod selection;
mod worker;

use self::recovery::PendingMicrophoneReplacement;
use self::selection::{MicrophoneDiagnosticIdentity, MicrophoneSelection};
use self::worker::MicrophoneWorker;
use crate::error::CaptureError;
use crate::streaming::AudioChunk;
use pocketstation::{AudioFrameDuration, DeviceSelector, Session, Source};

pub(crate) use self::health::{MicrophoneObservations, MicrophoneSignalState};
pub(crate) use self::recovery::MicrophoneReplacementReconciliation;

pub(crate) struct PocketStationMicrophoneStream {
    worker: MicrophoneWorker,
    selector: DeviceSelector,
    device_id: String,
    pending_replacement: Option<Box<PendingMicrophoneReplacement>>,
    receiver: crossbeam_channel::Receiver<AudioChunk>,
    device_name: String,
}

impl PocketStationMicrophoneStream {
    pub(crate) fn start(
        device_override: Option<&str>,
        resolved_default_name: &str,
    ) -> Result<Self, CaptureError> {
        let selection = MicrophoneSelection::resolve(device_override, resolved_default_name)?;
        Self::start_selection(selection)
    }

    pub(crate) fn start_fallback(
        previous_device_id: Option<&str>,
        resolved_default_name: &str,
    ) -> Result<Self, CaptureError> {
        let selection =
            MicrophoneSelection::recovery_fallback(previous_device_id, resolved_default_name)?;
        Self::start_selection(selection)
    }

    fn start_selection(selection: MicrophoneSelection) -> Result<Self, CaptureError> {
        let session = Session::builder()
            .audio_frame_duration(AudioFrameDuration::Ms10)
            .build();
        let microphone = session
            .capture(Source::microphone(selection.selector.clone()))
            .map_err(|error| capture_error("select PocketStation microphone", error))?;
        let output = session
            .polled_audio()
            .map_err(|error| capture_error("open PocketStation microphone output", error))?;
        microphone
            .send(output)
            .map_err(|error| capture_error("connect PocketStation microphone", error))?;
        let stem_id = microphone.id();

        let running = session
            .start()
            .map_err(|error| capture_error("start PocketStation microphone", error))?;
        let started = MicrophoneWorker::start(
            running,
            stem_id,
            MicrophoneDiagnosticIdentity::from(&selection),
        )?;

        Ok(Self {
            worker: started.worker,
            selector: selection.selector,
            device_id: selection.device_id,
            pending_replacement: None,
            receiver: started.receiver,
            device_name: selection.display_name,
        })
    }

    pub(crate) fn has_error(&self) -> bool {
        self.worker.has_error()
    }

    pub(crate) fn observations(&self) -> MicrophoneObservations {
        self.worker.observations()
    }

    pub(crate) fn device_id(&self) -> &str {
        &self.device_id
    }

    pub(crate) fn device_name(&self) -> &str {
        &self.device_name
    }

    pub(crate) fn receiver(&self) -> crossbeam_channel::Receiver<AudioChunk> {
        self.receiver.clone()
    }
}

fn capture_error(operation: &str, error: impl std::fmt::Display) -> CaptureError {
    CaptureError::Io(std::io::Error::other(format!("{operation}: {error}")))
}

#[cfg(test)]
mod tests;
