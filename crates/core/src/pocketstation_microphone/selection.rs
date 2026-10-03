use super::capture_error;
use pocketstation::{DeviceId, DeviceSelector, SourceKind};

#[derive(Debug, Clone)]
pub(super) struct MicrophoneSelection {
    pub(super) selector: DeviceSelector,
    pub(super) device_id: String,
    pub(super) display_name: String,
}

impl MicrophoneSelection {
    pub(super) fn resolve(
        device_override: Option<&str>,
        resolved_default_name: &str,
    ) -> Result<Self, crate::error::CaptureError> {
        use cpal::traits::{DeviceTrait, HostTrait};

        let explicit_request = device_override
            .map(str::trim)
            .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("default"));
        let (requested, exact_device_id) = if let Some(requested) = explicit_request {
            (requested.to_owned(), None)
        } else {
            let default = cpal::default_host().default_input_device().ok_or_else(|| {
                capture_error(
                    "select PocketStation microphone",
                    "no default input device is available",
                )
            })?;
            let device_id = default.id().map_err(|error| {
                capture_error(
                    "select PocketStation microphone",
                    format!("read default input device identity: {error}"),
                )
            })?;
            (
                resolved_default_name.to_owned(),
                Some(device_id.to_string()),
            )
        };

        select_discovered_microphone(
            discovered_microphones(),
            &requested,
            exact_device_id.as_deref(),
        )
    }

    pub(super) fn recovery_fallback(
        current_device_id: Option<&str>,
        resolved_default_name: &str,
    ) -> Result<Self, crate::error::CaptureError> {
        let default = Self::resolve(None, resolved_default_name).ok();
        choose_recovery_fallback(current_device_id, default).ok_or_else(|| {
            capture_error(
                "select PocketStation microphone fallback",
                "the system default input still resolves to the failed microphone; stop this recording, change or select the microphone, then start a new recording",
            )
        })
    }
}

pub(super) fn select_discovered_microphone(
    sources: Vec<MicrophoneSelection>,
    requested: &str,
    exact_device_id: Option<&str>,
) -> Result<MicrophoneSelection, crate::error::CaptureError> {
    let mut matching = sources.into_iter().filter(|source| {
        exact_device_id.map_or_else(
            || source.display_name.eq_ignore_ascii_case(requested) || source.device_id == requested,
            |device_id| source.device_id == device_id,
        )
    });
    let selected = matching.next().ok_or_else(|| {
        capture_error(
            "select PocketStation microphone",
            format!("no input device matches '{requested}'"),
        )
    })?;
    if matching.next().is_some() {
        return Err(capture_error(
            "select PocketStation microphone",
            format!("more than one input device matches '{requested}'"),
        ));
    }
    Ok(selected)
}

fn discovered_microphones() -> Vec<MicrophoneSelection> {
    pocketstation::discover_sources()
        .into_iter()
        .filter(|source| source.stable_id.kind == SourceKind::InputDevice)
        .filter_map(|source| {
            let device_id = source.device_uid?;
            Some(MicrophoneSelection {
                selector: DeviceSelector::id(DeviceId::new(device_id.clone())),
                device_id,
                display_name: source.name,
            })
        })
        .collect()
}

pub(super) fn choose_recovery_fallback(
    current_device_id: Option<&str>,
    default: Option<MicrophoneSelection>,
) -> Option<MicrophoneSelection> {
    let current_device_id = current_device_id?;
    default.filter(|default| default.device_id != current_device_id)
}

#[derive(Clone)]
pub(super) struct MicrophoneDiagnosticIdentity {
    pub(super) device_id: String,
    pub(super) display_name: String,
}

impl From<&MicrophoneSelection> for MicrophoneDiagnosticIdentity {
    fn from(selection: &MicrophoneSelection) -> Self {
        Self {
            device_id: selection.device_id.clone(),
            display_name: selection.display_name.clone(),
        }
    }
}
