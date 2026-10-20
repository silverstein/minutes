//! Observe the pinned pipeline without changing its inference path. Only fixed
//! stage labels and bounded elapsed milliseconds leave this subscriber.
use std::fmt::{self, Write};
use std::time::Instant;
use tracing::{field::Visit, Event, Subscriber};
use tracing_subscriber::{layer::Context, prelude::*, Layer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Segmentation,
    Embeddings,
}

struct Message {
    bytes: [u8; 64],
    len: usize,
    overflowed: bool,
}

impl Message {
    fn new() -> Self {
        Self {
            bytes: [0; 64],
            len: 0,
            overflowed: false,
        }
    }

    fn stage(&self) -> Option<Stage> {
        if self.overflowed {
            return None;
        }
        match &self.bytes[..self.len] {
            b"Segmentation complete" => Some(Stage::Segmentation),
            b"Embeddings complete" => Some(Stage::Embeddings),
            _ => None,
        }
    }
}

impl Write for Message {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if value.len() > self.bytes.len() - self.len {
            self.overflowed = true;
            return Err(fmt::Error);
        }
        self.bytes[self.len..self.len + value.len()].copy_from_slice(value.as_bytes());
        self.len += value.len();
        Ok(())
    }
}

impl Visit for Message {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self, "{value:?}");
        }
    }
}

struct StageLayer<F> {
    start: Instant,
    emit: F,
}

impl<S, F> Layer<S> for StageLayer<F>
where
    S: Subscriber,
    F: Fn(Stage, u64) + Send + Sync + 'static,
{
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        if event.metadata().target() != "speakrs::pipeline" {
            return;
        }
        let mut message = Message::new();
        event.record(&mut message);
        if let Some(stage) = message.stage() {
            (self.emit)(stage, elapsed_ms(self.start));
        }
    }
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().min(900_000) as u64
}

pub fn install() -> Result<Instant, tracing::subscriber::SetGlobalDefaultError> {
    let start = Instant::now();
    let layer = StageLayer {
        start,
        emit: |stage, milliseconds| {
            let (phase, timing) = match stage {
                Stage::Segmentation => ("segmentation_complete", "segmentation_end_ms"),
                Stage::Embeddings => ("embeddings_complete", "embeddings_end_ms"),
            };
            eprintln!("MINUTES_SPEAKER_PHASE {phase}");
            eprintln!("MINUTES_SPEAKER_TIMING {timing} {milliseconds}");
        },
    };
    let layer = layer.with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
        metadata.target() == "speakrs::pipeline" && *metadata.level() == tracing::Level::DEBUG
    }));
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(layer))?;
    Ok(start)
}

pub fn finished(start: Instant) {
    eprintln!(
        "MINUTES_SPEAKER_TIMING inference_end_ms {}",
        elapsed_ms(start)
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn real_tracing_events_emit_only_the_two_pinned_stage_labels() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let captured = observed.clone();
        let layer = StageLayer {
            start: Instant::now(),
            emit: move |stage, milliseconds| captured.lock().unwrap().push((stage, milliseconds)),
        };
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
            tracing::debug!(target: "speakrs::pipeline", windows = 123, "Segmentation complete");
            tracing::debug!(target: "speakrs::pipeline", private_path = "/private/audio", "Embeddings complete");
            tracing::debug!(target: "unrelated", "Segmentation complete");
            tracing::debug!(target: "speakrs::pipeline", "Segmentation complete private content");
            tracing::debug!(target: "speakrs::pipeline", "{}", "private".repeat(1000));
        });
        let observed = observed.lock().unwrap();
        assert_eq!(observed.len(), 2);
        assert_eq!(observed[0].0, Stage::Segmentation);
        assert_eq!(observed[1].0, Stage::Embeddings);
        assert!(observed
            .iter()
            .all(|(_, milliseconds)| *milliseconds <= 900_000));
    }

    #[test]
    fn overflow_cannot_become_an_accepted_prefix() {
        let mut message = Message::new();
        message.write_str("Segmentation complete").unwrap();
        assert!(message.write_str(&"x".repeat(100)).is_err());
        assert_eq!(message.stage(), None);
    }
}
