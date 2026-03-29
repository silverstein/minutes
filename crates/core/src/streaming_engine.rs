use crate::config::Config;
use crate::error::TranscribeError;
use crate::streaming_whisper::StreamingResult;

#[cfg(feature = "whisper")]
use whisper_rs::{WhisperContext, WhisperContextParameters};

pub enum StreamingEngine {
    #[cfg(feature = "whisper")]
    Whisper {
        ctx: whisper_rs::WhisperContext,
        streamer: crate::streaming_whisper::StreamingWhisper,
    },
    #[cfg(all(feature = "parakeet-coreml", target_os = "macos"))]
    ParakeetCoreml {
        streamer: crate::streaming_parakeet::StreamingParakeet,
    },
}

impl StreamingEngine {
    pub fn new_for_dictation(config: &Config) -> Result<Self, TranscribeError> {
        if config.transcription.engine == "parakeet-coreml" {
            #[cfg(all(feature = "parakeet-coreml", target_os = "macos"))]
            {
                return Ok(Self::ParakeetCoreml {
                    streamer: crate::streaming_parakeet::StreamingParakeet::new(config)?,
                });
            }

            #[cfg(not(all(feature = "parakeet-coreml", target_os = "macos")))]
            {
                return Err(TranscribeError::EngineNotAvailable(
                    "parakeet-coreml".into(),
                ));
            }
        }

        #[cfg(feature = "whisper")]
        {
            let model_path = crate::transcribe::resolve_model_path_for_dictation(config)?;
            let ctx = WhisperContext::new_with_params(
                model_path
                    .to_str()
                    .ok_or_else(|| TranscribeError::ModelLoadError("invalid path".into()))?,
                WhisperContextParameters::default(),
            )
            .map_err(|e| TranscribeError::ModelLoadError(format!("{}", e)))?;
            let streamer = crate::streaming_whisper::StreamingWhisper::new(
                config.transcription.language.clone(),
            );
            Ok(Self::Whisper { ctx, streamer })
        }

        #[cfg(not(feature = "whisper"))]
        {
            let _ = config;
            Err(TranscribeError::EngineNotAvailable("whisper".into()))
        }
    }

    pub fn new_for_live(config: &Config) -> Result<Self, TranscribeError> {
        if config.transcription.engine == "parakeet-coreml" {
            #[cfg(all(feature = "parakeet-coreml", target_os = "macos"))]
            {
                return Ok(Self::ParakeetCoreml {
                    streamer: crate::streaming_parakeet::StreamingParakeet::new(config)?,
                });
            }

            #[cfg(not(all(feature = "parakeet-coreml", target_os = "macos")))]
            {
                return Err(TranscribeError::EngineNotAvailable(
                    "parakeet-coreml".into(),
                ));
            }
        }

        #[cfg(feature = "whisper")]
        {
            let model_path = if config.live_transcript.model.is_empty() {
                crate::transcribe::resolve_model_path_for_dictation(config)?
            } else {
                crate::transcribe::resolve_model_path_by_name(
                    &config.live_transcript.model,
                    config,
                )?
            };
            let ctx = WhisperContext::new_with_params(
                model_path
                    .to_str()
                    .ok_or_else(|| TranscribeError::ModelLoadError("invalid path".into()))?,
                WhisperContextParameters::default(),
            )
            .map_err(|e| TranscribeError::ModelLoadError(format!("{}", e)))?;
            let streamer = crate::streaming_whisper::StreamingWhisper::new(
                config.transcription.language.clone(),
            );
            Ok(Self::Whisper { ctx, streamer })
        }

        #[cfg(not(feature = "whisper"))]
        {
            let _ = config;
            Err(TranscribeError::EngineNotAvailable("whisper".into()))
        }
    }

    #[cfg(any(
        feature = "whisper",
        all(feature = "parakeet-coreml", target_os = "macos")
    ))]
    pub fn feed(&mut self, samples: &[f32]) -> Option<StreamingResult> {
        match self {
            #[cfg(feature = "whisper")]
            Self::Whisper { ctx, streamer } => streamer.feed(samples, ctx),
            #[cfg(all(feature = "parakeet-coreml", target_os = "macos"))]
            Self::ParakeetCoreml { streamer } => streamer.feed(samples),
        }
    }

    #[cfg(not(any(
        feature = "whisper",
        all(feature = "parakeet-coreml", target_os = "macos")
    )))]
    pub fn feed(&mut self, samples: &[f32]) -> Option<StreamingResult> {
        let _ = samples;
        None
    }

    #[cfg(any(
        feature = "whisper",
        all(feature = "parakeet-coreml", target_os = "macos")
    ))]
    pub fn finalize(&mut self) -> Option<StreamingResult> {
        match self {
            #[cfg(feature = "whisper")]
            Self::Whisper { ctx, streamer } => streamer.finalize(ctx),
            #[cfg(all(feature = "parakeet-coreml", target_os = "macos"))]
            Self::ParakeetCoreml { streamer } => streamer.finalize(),
        }
    }

    #[cfg(not(any(
        feature = "whisper",
        all(feature = "parakeet-coreml", target_os = "macos")
    )))]
    pub fn finalize(&mut self) -> Option<StreamingResult> {
        None
    }

    #[cfg(any(
        feature = "whisper",
        all(feature = "parakeet-coreml", target_os = "macos")
    ))]
    pub fn reset(&mut self) {
        match self {
            #[cfg(feature = "whisper")]
            Self::Whisper { streamer, .. } => streamer.reset(),
            #[cfg(all(feature = "parakeet-coreml", target_os = "macos"))]
            Self::ParakeetCoreml { streamer } => streamer.reset(),
        }
    }

    #[cfg(not(any(
        feature = "whisper",
        all(feature = "parakeet-coreml", target_os = "macos")
    )))]
    pub fn reset(&mut self) {}

    #[cfg(any(
        feature = "whisper",
        all(feature = "parakeet-coreml", target_os = "macos")
    ))]
    pub fn duration_secs(&self) -> f64 {
        match self {
            #[cfg(feature = "whisper")]
            Self::Whisper { streamer, .. } => streamer.duration_secs(),
            #[cfg(all(feature = "parakeet-coreml", target_os = "macos"))]
            Self::ParakeetCoreml { streamer } => streamer.duration_secs(),
        }
    }

    #[cfg(not(any(
        feature = "whisper",
        all(feature = "parakeet-coreml", target_os = "macos")
    )))]
    pub fn duration_secs(&self) -> f64 {
        0.0
    }

    #[cfg(feature = "whisper")]
    pub fn take_whisper_ctx(self) -> Option<WhisperContext> {
        match self {
            Self::Whisper { ctx, .. } => Some(ctx),
            #[cfg(all(feature = "parakeet-coreml", target_os = "macos"))]
            Self::ParakeetCoreml { .. } => None,
        }
    }
}
