//! Presentation policy for desktop dictation, which accumulates phrases until
//! the user finishes. Segment transcription is not a delivery acknowledgement.

pub fn overlay_state_for_event(state: &str, stop_requested: bool) -> Option<&str> {
    match state {
        "" | "success" => None,
        "processing" if !stop_requested => None,
        "listening" | "accumulating" if stop_requested => Some("processing"),
        _ => Some(state),
    }
}

#[cfg(test)]
mod tests {
    use super::overlay_state_for_event;

    #[test]
    fn thinking_pause_does_not_claim_capture_finished() {
        assert_eq!(overlay_state_for_event("processing", false), None);
        assert_eq!(overlay_state_for_event("success", false), None);
        assert_eq!(
            overlay_state_for_event("listening", false),
            Some("listening")
        );
        assert_eq!(
            overlay_state_for_event("accumulating", false),
            Some("accumulating")
        );
    }

    #[test]
    fn finishing_never_returns_to_listening() {
        for state in ["processing", "listening", "accumulating"] {
            assert_eq!(overlay_state_for_event(state, true), Some("processing"));
        }
    }

    #[test]
    fn transcript_success_is_never_delivery_confirmation() {
        for stop_requested in [false, true] {
            assert_eq!(overlay_state_for_event("success", stop_requested), None);
        }
    }

    #[test]
    fn failure_and_cancellation_remain_visible() {
        for state in ["error", "model-missing", "cancelled", "yielded"] {
            for stop_requested in [false, true] {
                assert_eq!(overlay_state_for_event(state, stop_requested), Some(state));
            }
        }
    }

    #[test]
    fn non_state_engine_events_stay_silent() {
        assert_eq!(overlay_state_for_event("", false), None);
        assert_eq!(overlay_state_for_event("", true), None);
    }
}
