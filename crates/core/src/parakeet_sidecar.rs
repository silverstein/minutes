#[cfg(all(feature = "parakeet", unix))]
mod imp {
    use crate::config::Config;
    use crate::error::TranscribeError;
    use crate::transcribe::{
        parakeet_transcript_from_segments, write_wav_16k_mono, ParakeetCliSegment,
        ParakeetCliTranscript,
    };
    use serde::{Deserialize, Serialize};
    use std::collections::{HashSet, VecDeque};
    use std::fmt;
    use std::fs;
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};

    const LOG_PREFIX: &str = "parakeet-sidecar:";
    const STDERR_RING_CAPACITY: usize = 200;
    const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
    const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
    const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
    const DEFAULT_FP16_CRASH_WINDOW: Duration = Duration::from_secs(60);
    const DEFAULT_MAX_REQUEST_TIMEOUT: Duration = Duration::from_secs(30 * 60);
    const DEFAULT_MIN_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
    const HEALTHCHECK_WAV_FILENAME: &str = "parakeet-sidecar-healthcheck.wav";
    const FP16_CRASH_SIGNATURES: &[&str] = &[
        "MPSGraph",
        "requires the same element type",
        "original module failed verification",
        "'mps.add' op requires the same element type for all operands and results",
    ];

    #[derive(Debug, Clone)]
    pub struct SidecarLaunchSpec {
        pub server_binary: PathBuf,
        pub socket_path: PathBuf,
        pub model_path: PathBuf,
        pub vocab_path: PathBuf,
        pub model_id: String,
        pub use_gpu: bool,
        pub use_fp16: bool,
        pub vad_path: Option<PathBuf>,
    }

    impl SidecarLaunchSpec {
        fn matches(&self, other: &Self) -> bool {
            self.server_binary == other.server_binary
                && self.model_path == other.model_path
                && self.vocab_path == other.vocab_path
                && self.model_id == other.model_id
                && self.use_gpu == other.use_gpu
                && self.use_fp16 == other.use_fp16
                && self.vad_path == other.vad_path
        }

        fn display_key(&self) -> String {
            format!(
                "{}:{}:gpu={}:fp16={}",
                self.server_binary.display(),
                self.model_id,
                self.use_gpu,
                self.use_fp16
            )
        }

        fn fp16_downgrade_key(&self) -> String {
            format!(
                "{}:{}:{}:{}:{}:{}",
                self.server_binary.display(),
                self.model_path.display(),
                self.vocab_path.display(),
                self.model_id,
                self.use_gpu,
                self.vad_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default()
            )
        }
    }

    #[derive(Debug, Clone, Serialize)]
    pub struct SidecarRequest {
        pub request_id: String,
        pub audio_path: String,
        pub decoder: String,
        pub timestamps: bool,
        pub use_vad: bool,
        pub beam_width: i32,
        pub lm_path: String,
        pub lm_weight: f32,
        pub boost_phrases: Vec<String>,
        pub boost_score: f32,
    }

    #[derive(Debug, Clone)]
    pub struct SidecarTranscriptResult {
        pub transcript: ParakeetCliTranscript,
        pub elapsed_ms: u64,
        pub first_request_on_process: bool,
        pub effective_fp16: bool,
    }

    #[derive(Debug, Clone, Deserialize)]
    struct SidecarWordTimestamp {
        word: String,
        start: f64,
        end: f64,
        confidence: Option<f32>,
    }

    #[derive(Debug, Clone, Deserialize)]
    struct SidecarResponse {
        ok: bool,
        request_id: Option<String>,
        text: Option<String>,
        elapsed_ms: Option<u64>,
        word_timestamps: Option<Vec<SidecarWordTimestamp>>,
        error: Option<String>,
    }

    #[derive(Debug, Clone)]
    pub struct SidecarError {
        message: String,
    }

    impl SidecarError {
        fn new(message: impl Into<String>) -> Self {
            Self {
                message: message.into(),
            }
        }
    }

    impl fmt::Display for SidecarError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.message)
        }
    }

    impl std::error::Error for SidecarError {}

    #[derive(Debug, Clone, Copy)]
    struct SidecarTimeouts {
        startup: Duration,
        connect: Duration,
        shutdown: Duration,
        fp16_crash_window: Duration,
        max_request: Duration,
        min_request: Duration,
    }

    impl Default for SidecarTimeouts {
        fn default() -> Self {
            Self {
                startup: DEFAULT_STARTUP_TIMEOUT,
                connect: DEFAULT_CONNECT_TIMEOUT,
                shutdown: DEFAULT_SHUTDOWN_TIMEOUT,
                fp16_crash_window: DEFAULT_FP16_CRASH_WINDOW,
                max_request: DEFAULT_MAX_REQUEST_TIMEOUT,
                min_request: DEFAULT_MIN_REQUEST_TIMEOUT,
            }
        }
    }

    #[derive(Debug)]
    struct RunningSidecar {
        spec: SidecarLaunchSpec,
        child: Child,
        started_at: Instant,
        stderr_lines: Arc<Mutex<VecDeque<String>>>,
        stderr_thread: Option<thread::JoinHandle<()>>,
        requests_served: u64,
    }

    #[derive(Debug)]
    enum SidecarState {
        Disabled,
        Cold,
        Starting {
            spec: SidecarLaunchSpec,
            started_at: Instant,
        },
        Healthy(RunningSidecar),
        SubprocessOnly {
            spec: Option<SidecarLaunchSpec>,
            reason: String,
            since: Instant,
        },
        Stopping,
    }

    #[derive(Debug)]
    enum LaunchFailureAction {
        DowngradeToFp32,
        Fallback,
    }

    #[derive(Debug)]
    struct StartFailure {
        message: String,
        stderr_lines: Vec<String>,
        uptime: Duration,
    }

    #[derive(Debug)]
    pub struct ParakeetSidecarManager {
        state: SidecarState,
        request_counter: u64,
        timeouts: SidecarTimeouts,
        fp16_downgrade_keys: HashSet<String>,
    }

    impl Default for ParakeetSidecarManager {
        fn default() -> Self {
            Self {
                state: SidecarState::Cold,
                request_counter: 0,
                timeouts: SidecarTimeouts::default(),
                fp16_downgrade_keys: HashSet::new(),
            }
        }
    }

    impl ParakeetSidecarManager {
        pub fn transcribe(
            &mut self,
            spec: SidecarLaunchSpec,
            request: SidecarRequest,
            config: &Config,
            audio_duration_secs: f64,
        ) -> Result<SidecarTranscriptResult, SidecarError> {
            if let SidecarState::Disabled = self.state {
                return Err(SidecarError::new(format!(
                    "{LOG_PREFIX} disabled for this process"
                )));
            }

            self.ensure_started(spec.clone(), config)?;

            match self.transcribe_once(&request, config, audio_duration_secs) {
                Ok(result) => Ok(result),
                Err(first_error) => {
                    if is_non_restartable_request_error(&first_error) {
                        return Err(first_error);
                    }
                    let retry_spec = self.retry_spec_after_live_failure(&spec, &first_error);
                    tracing::warn!(
                        "{} live request failed — restarting once before fallback: {}",
                        LOG_PREFIX,
                        first_error
                    );
                    self.stop_running_sidecar();
                    self.state = SidecarState::Cold;
                    self.ensure_started(retry_spec.clone(), config)?;
                    match self.transcribe_once(&request, config, audio_duration_secs) {
                        Ok(result) => Ok(result),
                        Err(second_error) => {
                            let reason = format!(
                                "{LOG_PREFIX} request failed after one restart: {}",
                                second_error
                            );
                            self.state = SidecarState::SubprocessOnly {
                                spec: Some(spec),
                                reason: reason.clone(),
                                since: Instant::now(),
                            };
                            Err(SidecarError::new(reason))
                        }
                    }
                }
            }
        }

        pub fn ensure_started(
            &mut self,
            mut spec: SidecarLaunchSpec,
            config: &Config,
        ) -> Result<(), SidecarError> {
            self.apply_sticky_fp16_downgrade(&mut spec);

            if let SidecarState::Healthy(running) = &mut self.state {
                if running.spec.matches(&spec) && process_is_alive(&mut running.child) {
                    return Ok(());
                }
            }
            if let SidecarState::SubprocessOnly {
                spec: Some(failed_spec),
                reason,
                since,
            } = &self.state
            {
                if failed_spec.matches(&spec) {
                    return Err(SidecarError::new(format!(
                        "{LOG_PREFIX} subprocess fallback remains active since {:?}: {}",
                        since.elapsed(),
                        reason
                    )));
                }
            }

            if matches!(self.state, SidecarState::Healthy(_)) {
                self.stop_running_sidecar();
            }

            let original_spec = spec.clone();
            let mut attempt_spec = spec;

            loop {
                self.state = SidecarState::Starting {
                    spec: attempt_spec.clone(),
                    started_at: Instant::now(),
                };
                match self.start_once(attempt_spec.clone(), config) {
                    Ok(running) => {
                        tracing::info!(
                            "{} healthy socket={} spec={}",
                            LOG_PREFIX,
                            running.spec.socket_path.display(),
                            running.spec.display_key()
                        );
                        self.state = SidecarState::Healthy(running);
                        return Ok(());
                    }
                    Err(failure) => {
                        let action = classify_start_failure(
                            &attempt_spec,
                            &failure,
                            self.timeouts.fp16_crash_window,
                        );
                        match action {
                            LaunchFailureAction::DowngradeToFp32 => {
                                self.remember_fp16_downgrade(&attempt_spec);
                                tracing::warn!(
                                    "{} detected fp16 startup crash signature — retrying in fp32",
                                    LOG_PREFIX
                                );
                                attempt_spec.use_fp16 = false;
                            }
                            LaunchFailureAction::Fallback => {
                                let reason = format!("{} {}", LOG_PREFIX, failure.message);
                                self.state = SidecarState::SubprocessOnly {
                                    spec: Some(original_spec),
                                    reason: reason.clone(),
                                    since: Instant::now(),
                                };
                                return Err(SidecarError::new(reason));
                            }
                        }
                    }
                }
            }
        }

        fn transcribe_once(
            &mut self,
            request: &SidecarRequest,
            config: &Config,
            audio_duration_secs: f64,
        ) -> Result<SidecarTranscriptResult, SidecarError> {
            let request_timeout = request_timeout(audio_duration_secs, self.timeouts);
            let (spec, first_request_on_process) = match &mut self.state {
                SidecarState::Healthy(running) => {
                    if !process_is_alive(&mut running.child) {
                        return Err(SidecarError::new(format!(
                            "{LOG_PREFIX} child exited before request dispatch"
                        )));
                    }
                    let first = running.requests_served == 0;
                    running.requests_served += 1;
                    (running.spec.clone(), first)
                }
                _ => {
                    return Err(SidecarError::new(format!(
                        "{LOG_PREFIX} requested transcription while not healthy"
                    )));
                }
            };

            let response_line = request_sidecar(
                &spec.socket_path,
                request,
                self.timeouts.connect,
                request_timeout,
            )
            .map_err(|error| SidecarError::new(format!("{LOG_PREFIX} {}", error)))?;

            let response: SidecarResponse =
                serde_json::from_str(&response_line).map_err(|error| {
                    SidecarError::new(format!("{LOG_PREFIX} malformed JSON response: {}", error))
                })?;
            let elapsed_ms = response.elapsed_ms.unwrap_or_default();

            if response.request_id.as_deref() != Some(request.request_id.as_str()) {
                return Err(SidecarError::new(format!(
                    "{LOG_PREFIX} mismatched request_id in sidecar response"
                )));
            }

            if !response.ok {
                return Err(SidecarError::new(format!(
                    "{LOG_PREFIX} server error: {}",
                    response.error.unwrap_or_else(|| "unknown error".into())
                )));
            }

            let transcript = response_to_parakeet_transcript(&response_line, response, config)
                .map_err(|error| SidecarError::new(format!("{LOG_PREFIX} {}", error)))?;

            Ok(SidecarTranscriptResult {
                transcript,
                elapsed_ms,
                first_request_on_process,
                effective_fp16: spec.use_fp16,
            })
        }

        fn start_once(
            &mut self,
            spec: SidecarLaunchSpec,
            _config: &Config,
        ) -> Result<RunningSidecar, StartFailure> {
            if let Some(parent) = spec.socket_path.parent() {
                fs::create_dir_all(parent).map_err(|error| StartFailure {
                    message: format!(
                        "could not create sidecar socket directory {}: {}",
                        parent.display(),
                        error
                    ),
                    stderr_lines: Vec::new(),
                    uptime: Duration::ZERO,
                })?;
            }
            let _ = fs::remove_file(&spec.socket_path);

            let mut command = Command::new(&spec.server_binary);
            command
                .arg(&spec.socket_path)
                .arg(&spec.model_path)
                .arg(&spec.vocab_path)
                .args(["--model", spec.model_id.as_str()])
                .stderr(Stdio::piped())
                .stdout(Stdio::null())
                .stdin(Stdio::null());
            if spec.use_gpu {
                command.arg("--gpu");
            }
            if spec.use_fp16 {
                command.arg("--fp16");
            }
            if let Some(vad_path) = &spec.vad_path {
                command.arg("--vad").arg(vad_path);
            }

            tracing::info!(
                "{} starting binary={} socket={} model={} gpu={} fp16={} vad={}",
                LOG_PREFIX,
                spec.server_binary.display(),
                spec.socket_path.display(),
                spec.model_id,
                spec.use_gpu,
                spec.use_fp16,
                spec.vad_path.is_some()
            );

            let started_at = Instant::now();
            let mut child = command.spawn().map_err(|error| StartFailure {
                message: format!(
                    "failed to spawn sidecar binary {}: {}",
                    spec.server_binary.display(),
                    error
                ),
                stderr_lines: Vec::new(),
                uptime: Duration::ZERO,
            })?;

            let stderr_lines = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_RING_CAPACITY)));
            let stderr_thread = child
                .stderr
                .take()
                .map(|stderr| spawn_stderr_drain(stderr, stderr_lines.clone()));

            let mut running = RunningSidecar {
                spec,
                child,
                started_at,
                stderr_lines,
                stderr_thread,
                requests_served: 0,
            };

            match self.health_check(&mut running) {
                Ok(()) => Ok(running),
                Err(message) => {
                    let stderr = running.recent_stderr_lines();
                    let uptime = running.started_at.elapsed();
                    self.terminate_running_sidecar(&mut running);
                    Err(StartFailure {
                        message,
                        stderr_lines: stderr,
                        uptime,
                    })
                }
            }
        }

        fn retry_spec_after_live_failure(
            &mut self,
            spec: &SidecarLaunchSpec,
            error: &SidecarError,
        ) -> SidecarLaunchSpec {
            let failure = match &self.state {
                SidecarState::Healthy(running) => StartFailure {
                    message: error.to_string(),
                    stderr_lines: running.recent_stderr_lines(),
                    uptime: running.started_at.elapsed(),
                },
                _ => {
                    return spec.clone();
                }
            };

            if matches!(
                classify_start_failure(spec, &failure, self.timeouts.fp16_crash_window),
                LaunchFailureAction::DowngradeToFp32
            ) {
                let mut downgraded = spec.clone();
                self.remember_fp16_downgrade(spec);
                downgraded.use_fp16 = false;
                tracing::warn!(
                    "{} live request crash matched fp16 MPSGraph signature — retrying in fp32",
                    LOG_PREFIX
                );
                downgraded
            } else {
                spec.clone()
            }
        }

        fn remember_fp16_downgrade(&mut self, spec: &SidecarLaunchSpec) {
            self.fp16_downgrade_keys.insert(spec.fp16_downgrade_key());
        }

        fn apply_sticky_fp16_downgrade(&self, spec: &mut SidecarLaunchSpec) {
            if spec.use_fp16
                && self
                    .fp16_downgrade_keys
                    .contains(&spec.fp16_downgrade_key())
            {
                tracing::warn!(
                    "{} reusing prior fp16 downgrade decision for model={}",
                    LOG_PREFIX,
                    spec.model_id
                );
                spec.use_fp16 = false;
            }
        }

        fn health_check(&mut self, running: &mut RunningSidecar) -> Result<(), String> {
            let deadline = Instant::now() + self.timeouts.startup;
            while !running.spec.socket_path.exists() {
                if !process_is_alive(&mut running.child) {
                    let status = running.child.try_wait().ok().flatten();
                    return Err(format!(
                        "sidecar exited before socket became ready{}",
                        format_exit_status(status)
                    ));
                }
                if Instant::now() >= deadline {
                    return Err(format!(
                        "health check timed out waiting for socket {}",
                        running.spec.socket_path.display()
                    ));
                }
                thread::sleep(Duration::from_millis(50));
            }

            let request = SidecarRequest {
                request_id: "__minutes_healthcheck__".into(),
                audio_path: healthcheck_wav_path()
                    .map_err(|error| error.to_string())?
                    .display()
                    .to_string(),
                decoder: "tdt".into(),
                timestamps: false,
                use_vad: false,
                beam_width: 8,
                lm_path: String::new(),
                lm_weight: 0.5,
                boost_phrases: Vec::new(),
                boost_score: 0.0,
            };
            let response_line = request_sidecar(
                &running.spec.socket_path,
                &request,
                self.timeouts.connect,
                self.timeouts.startup,
            )
            .map_err(|error| format!("health check failed: {}", error))?;
            let response: SidecarResponse = serde_json::from_str(&response_line)
                .map_err(|error| format!("health check returned malformed JSON: {}", error))?;
            if !response.ok {
                return Err(format!(
                    "health check returned server error: {}",
                    response.error.unwrap_or_else(|| "unknown error".into())
                ));
            }
            Ok(())
        }

        pub fn shutdown(&mut self) {
            self.stop_running_sidecar();
            self.state = SidecarState::Disabled;
        }

        fn stop_running_sidecar(&mut self) {
            let previous = std::mem::replace(&mut self.state, SidecarState::Stopping);
            match previous {
                SidecarState::Healthy(mut running) => {
                    tracing::info!(
                        "{} stopping socket={} spec={}",
                        LOG_PREFIX,
                        running.spec.socket_path.display(),
                        running.spec.display_key()
                    );
                    self.terminate_running_sidecar(&mut running);
                    self.state = SidecarState::Cold;
                }
                SidecarState::Starting { spec, started_at } => {
                    tracing::debug!(
                        "{} abandoning in-flight start spec={} after {:?}",
                        LOG_PREFIX,
                        spec.display_key(),
                        started_at.elapsed()
                    );
                    self.state = SidecarState::Cold;
                }
                other => {
                    self.state = other;
                }
            }
        }

        fn terminate_running_sidecar(&self, running: &mut RunningSidecar) {
            let pid = running.child.id();
            #[allow(clippy::cast_possible_wrap)]
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }

            let deadline = Instant::now() + self.timeouts.shutdown;
            loop {
                match running.child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {
                        if Instant::now() >= deadline {
                            break;
                        }
                        thread::sleep(Duration::from_millis(50));
                    }
                    Err(error) => {
                        tracing::warn!(
                            "{} failed to poll child {} during shutdown: {}",
                            LOG_PREFIX,
                            pid,
                            error
                        );
                        break;
                    }
                }
            }

            match running.child.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    if let Err(error) = running.child.kill() {
                        tracing::warn!("{} failed to SIGKILL child {}: {}", LOG_PREFIX, pid, error);
                    }
                    let _ = running.child.wait();
                }
                Err(error) => {
                    tracing::warn!(
                        "{} failed to poll child {} before forced kill: {}",
                        LOG_PREFIX,
                        pid,
                        error
                    );
                }
            }

            if let Some(handle) = running.stderr_thread.take() {
                let _ = handle.join();
            }
            let _ = fs::remove_file(&running.spec.socket_path);
        }
    }

    impl RunningSidecar {
        fn recent_stderr_lines(&self) -> Vec<String> {
            let guard = self
                .stderr_lines
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.iter().cloned().collect()
        }
    }

    fn spawn_stderr_drain(
        stderr: impl Read + Send + 'static,
        lines: Arc<Mutex<VecDeque<String>>>,
    ) -> thread::JoinHandle<()> {
        thread::Builder::new()
            .name("parakeet-sidecar-stderr".into())
            .spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines() {
                    match line {
                        Ok(line) => {
                            tracing::warn!("{} {}", LOG_PREFIX, line);
                            let mut guard = lines
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            if guard.len() >= STDERR_RING_CAPACITY {
                                guard.pop_front();
                            }
                            guard.push_back(line);
                        }
                        Err(error) => {
                            tracing::warn!("{} failed reading child stderr: {}", LOG_PREFIX, error);
                            break;
                        }
                    }
                }
            })
            .unwrap_or_else(|error| {
                tracing::warn!("{} failed to spawn stderr reader: {}", LOG_PREFIX, error);
                thread::spawn(|| {})
            })
    }

    fn process_is_alive(child: &mut Child) -> bool {
        match child.try_wait() {
            Ok(Some(_)) => false,
            Ok(None) => true,
            Err(_) => false,
        }
    }

    fn format_exit_status(status: Option<std::process::ExitStatus>) -> String {
        match status {
            Some(status) => format!(" (status: {})", status),
            None => String::new(),
        }
    }

    fn request_timeout(audio_duration_secs: f64, timeouts: SidecarTimeouts) -> Duration {
        let scaled = Duration::from_secs_f64((audio_duration_secs * 2.0).max(0.0));
        scaled.clamp(timeouts.min_request, timeouts.max_request)
    }

    fn request_sidecar(
        socket_path: &Path,
        request: &SidecarRequest,
        connect_timeout: Duration,
        read_timeout: Duration,
    ) -> io::Result<String> {
        let mut stream = UnixStream::connect(socket_path)?;
        stream.set_read_timeout(Some(read_timeout))?;
        stream.set_write_timeout(Some(connect_timeout))?;

        let line = serde_json::to_string(request)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        write_complete_line(&mut stream, &line)?;
        read_line_response(&mut stream)
    }

    fn write_complete_line(stream: &mut UnixStream, line: &str) -> io::Result<()> {
        let mut bytes = line.as_bytes().to_vec();
        bytes.push(b'\n');
        let mut written = 0usize;
        while written < bytes.len() {
            let count = stream.write(&bytes[written..])?;
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "sidecar socket closed during write",
                ));
            }
            written += count;
        }
        Ok(())
    }

    fn read_line_response(stream: &mut UnixStream) -> io::Result<String> {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "sidecar socket closed before newline-delimited response",
                    ));
                }
                Ok(count) => {
                    buffer.extend_from_slice(&chunk[..count]);
                    if let Some(position) = buffer.iter().position(|byte| *byte == b'\n') {
                        let line = String::from_utf8_lossy(&buffer[..position]).to_string();
                        return Ok(line);
                    }
                }
                Err(error)
                    if error.kind() == io::ErrorKind::Interrupted
                        || error.kind() == io::ErrorKind::WouldBlock =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn response_to_parakeet_transcript(
        raw_line: &str,
        response: SidecarResponse,
        config: &Config,
    ) -> Result<ParakeetCliTranscript, TranscribeError> {
        let server_text = response.text.ok_or_else(|| {
            TranscribeError::ParakeetFailed("sidecar success response missing text".into())
        })?;
        let words = response.word_timestamps.ok_or_else(|| {
            TranscribeError::ParakeetFailed(
                "sidecar success response missing word_timestamps".into(),
            )
        })?;
        let segments: Vec<ParakeetCliSegment> = words
            .into_iter()
            .map(|word| ParakeetCliSegment {
                start_secs: word.start,
                end_secs: word.end,
                confidence: word.confidence,
                text: word.word,
            })
            .collect();

        let parsed = parakeet_transcript_from_segments(raw_line, segments, config)?;
        if parsed.transcript.trim().is_empty() && !server_text.trim().is_empty() {
            return Err(TranscribeError::EmptyTranscript(
                config.transcription.min_words,
            ));
        }
        Ok(parsed)
    }

    fn is_non_restartable_request_error(error: &SidecarError) -> bool {
        error.to_string().contains("transcription produced no text")
    }

    fn classify_start_failure(
        spec: &SidecarLaunchSpec,
        failure: &StartFailure,
        fp16_crash_window: Duration,
    ) -> LaunchFailureAction {
        if spec.use_fp16
            && failure.uptime <= fp16_crash_window
            && failure
                .stderr_lines
                .iter()
                .any(|line| FP16_CRASH_SIGNATURES.iter().any(|sig| line.contains(sig)))
        {
            LaunchFailureAction::DowngradeToFp32
        } else {
            LaunchFailureAction::Fallback
        }
    }

    fn healthcheck_wav_path() -> io::Result<PathBuf> {
        static HEALTHCHECK_PATH: OnceLock<PathBuf> = OnceLock::new();
        if let Some(path) = HEALTHCHECK_PATH.get() {
            if path.exists() {
                return Ok(path.clone());
            }
        }

        let tmp_dir = Config::minutes_dir().join("tmp");
        fs::create_dir_all(&tmp_dir)?;
        let path = tmp_dir.join(HEALTHCHECK_WAV_FILENAME);
        if !path.exists() {
            let silence = vec![0.0f32; 16000 / 4];
            write_wav_16k_mono(&path, &silence)
                .map_err(|error| io::Error::other(error.to_string()))?;
            let mut permissions = fs::metadata(&path)?.permissions();
            permissions.set_mode(0o600);
            let _ = fs::set_permissions(&path, permissions);
        }
        let _ = HEALTHCHECK_PATH.set(path.clone());
        Ok(path)
    }

    fn next_request_id(manager: &mut ParakeetSidecarManager) -> String {
        manager.request_counter += 1;
        format!("minutes-{}", manager.request_counter)
    }

    pub fn build_launch_spec(
        config: &Config,
        model_path: &Path,
        vocab_path: &Path,
        vad_path: Option<&Path>,
    ) -> Result<SidecarLaunchSpec, SidecarError> {
        let server_binary = resolve_server_binary(&config.transcription.parakeet_binary)
            .ok_or_else(|| {
                SidecarError::new(format!(
                    "{LOG_PREFIX} could not resolve example-server binary. \
                     Set MINUTES_PARAKEET_SERVER_BINARY or put example-server on PATH"
                ))
            })?;

        let socket_path = Config::minutes_dir().join("tmp").join(format!(
            "parakeet-sidecar-{}-{}.sock",
            std::process::id(),
            config.transcription.parakeet_model
        ));

        Ok(SidecarLaunchSpec {
            server_binary,
            socket_path,
            model_path: model_path.to_path_buf(),
            vocab_path: vocab_path.to_path_buf(),
            model_id: config.transcription.parakeet_model.clone(),
            use_gpu: cfg!(all(target_os = "macos", target_arch = "aarch64")),
            use_fp16: cfg!(all(target_os = "macos", target_arch = "aarch64"))
                && config.transcription.parakeet_fp16,
            vad_path: vad_path.map(Path::to_path_buf),
        })
    }

    pub fn build_request(
        manager: &mut ParakeetSidecarManager,
        config: &Config,
        wav_path: &Path,
        use_vad: bool,
    ) -> Result<SidecarRequest, SidecarError> {
        let audio_path = wav_path.to_str().ok_or_else(|| {
            SidecarError::new(format!("{LOG_PREFIX} temp WAV path is not valid UTF-8"))
        })?;

        Ok(SidecarRequest {
            request_id: next_request_id(manager),
            audio_path: audio_path.to_string(),
            decoder: "tdt".into(),
            timestamps: true,
            use_vad,
            beam_width: 8,
            lm_path: String::new(),
            lm_weight: 0.5,
            boost_phrases: load_boost_phrases(config),
            boost_score: config.transcription.parakeet_boost_score,
        })
    }

    fn load_boost_phrases(config: &Config) -> Vec<String> {
        let limit = config.transcription.parakeet_boost_limit;
        if limit == 0 {
            return Vec::new();
        }
        match crate::graph::parakeet_boost_phrases(limit) {
            Ok(phrases) => phrases,
            Err(error) => {
                tracing::debug!("{} could not load boost phrases: {}", LOG_PREFIX, error);
                Vec::new()
            }
        }
    }

    pub fn resolve_server_binary(parakeet_binary: &str) -> Option<PathBuf> {
        if let Ok(explicit) = std::env::var("MINUTES_PARAKEET_SERVER_BINARY") {
            let explicit = PathBuf::from(explicit);
            if explicit.exists() {
                return Some(explicit);
            }
        }

        if let Ok(path_binary) = which::which("example-server") {
            return Some(path_binary);
        }

        let parakeet_path = PathBuf::from(parakeet_binary);
        if parakeet_path.is_absolute() {
            if let Some(parent) = parakeet_path.parent() {
                let sibling = parent.join("example-server");
                if sibling.exists() {
                    return Some(sibling);
                }
            }
            return None;
        }

        if let Ok(resolved_parakeet) = which::which(parakeet_binary) {
            if let Some(parent) = resolved_parakeet.parent() {
                let sibling = parent.join("example-server");
                if sibling.exists() {
                    return Some(sibling);
                }
            }
        }

        None
    }

    fn global_manager() -> &'static Mutex<ParakeetSidecarManager> {
        static MANAGER: OnceLock<Mutex<ParakeetSidecarManager>> = OnceLock::new();
        MANAGER.get_or_init(|| Mutex::new(ParakeetSidecarManager::default()))
    }

    pub fn transcribe_via_global_sidecar(
        config: &Config,
        model_path: &Path,
        vocab_path: &Path,
        vad_path: Option<&Path>,
        wav_path: &Path,
        audio_duration_secs: f64,
    ) -> Result<SidecarTranscriptResult, SidecarError> {
        let spec = build_launch_spec(config, model_path, vocab_path, vad_path)?;
        let manager = global_manager();
        let mut guard = manager
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let request = build_request(&mut guard, config, wav_path, vad_path.is_some())?;
        guard.transcribe(spec, request, config, audio_duration_secs)
    }

    pub fn warmup_global_sidecar(
        config: &Config,
        model_path: &Path,
        vocab_path: &Path,
        vad_path: Option<&Path>,
    ) -> Result<bool, SidecarError> {
        let spec = build_launch_spec(config, model_path, vocab_path, vad_path)?;
        let manager = global_manager();
        let mut guard = manager
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let already_healthy =
            matches!(&guard.state, SidecarState::Healthy(running) if running.spec.matches(&spec));
        guard.ensure_started(spec, config)?;
        Ok(!already_healthy)
    }

    pub fn shutdown_global_parakeet_sidecar() {
        let manager = global_manager();
        let mut guard = manager
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.shutdown();
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use tempfile::TempDir;

        fn make_temp_script(contents: &str) -> PathBuf {
            let base = std::env::temp_dir().join(format!(
                "minutes-sidecar-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            fs::create_dir_all(&base).unwrap();
            let path = base.join("fake-server.sh");
            fs::write(&path, contents).unwrap();
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
            path
        }

        fn make_spec(binary: PathBuf, use_fp16: bool) -> SidecarLaunchSpec {
            SidecarLaunchSpec {
                server_binary: binary,
                socket_path: Config::minutes_dir().join("tmp").join(format!(
                    "test-{}-{}.sock",
                    std::process::id(),
                    use_fp16
                )),
                model_path: PathBuf::from("/tmp/model.safetensors"),
                vocab_path: PathBuf::from("/tmp/vocab.txt"),
                model_id: "tdt-600m".into(),
                use_gpu: true,
                use_fp16,
                vad_path: None,
            }
        }

        #[test]
        fn fp16_crash_signature_requests_downgrade() {
            let spec = make_spec(PathBuf::from("/tmp/example-server"), true);
            let failure = StartFailure {
                message: "startup failed".into(),
                stderr_lines: FP16_CRASH_SIGNATURES
                    .iter()
                    .map(|value| value.to_string())
                    .collect(),
                uptime: Duration::from_secs(5),
            };
            assert!(matches!(
                classify_start_failure(&spec, &failure, Duration::from_secs(60)),
                LaunchFailureAction::DowngradeToFp32
            ));
        }

        #[test]
        fn fp16_crash_path_retries_without_fp16() {
            let mut manager = ParakeetSidecarManager::default();
            manager.timeouts.startup = Duration::from_millis(100);
            let config = Config::default();
            let script = make_temp_script(
                "#!/bin/sh\n\
                 case \"$*\" in\n\
                   *--fp16*)\n\
                     echo \"MPSGraph\" >&2\n\
                     echo \"requires the same element type\" >&2\n\
                     echo \"original module failed verification\" >&2\n\
                     exit 1\n\
                     ;;\n\
                   *)\n\
                     sleep 2\n\
                     ;;\n\
                 esac\n",
            );
            let spec = make_spec(script, true);

            let result = manager.ensure_started(spec, &config);
            assert!(result.is_err());
            let error_text = result.unwrap_err().to_string();
            assert!(
                error_text.contains("health check timed out"),
                "expected downgrade retry to reach the second launch timeout, got: {}",
                error_text
            );
            assert!(matches!(manager.state, SidecarState::SubprocessOnly { .. }));
        }

        #[test]
        fn spawn_failure_transitions_to_subprocess_only() {
            let mut manager = ParakeetSidecarManager::default();
            manager.timeouts.startup = Duration::from_millis(50);
            let config = Config::default();
            let spec = make_spec(PathBuf::from("/definitely/missing/example-server"), false);

            let result = manager.ensure_started(spec.clone(), &config);
            assert!(result.is_err());
            assert!(matches!(manager.state, SidecarState::SubprocessOnly { .. }));
        }

        #[test]
        fn health_check_timeout_transitions_to_subprocess_only() {
            let mut manager = ParakeetSidecarManager::default();
            manager.timeouts.startup = Duration::from_millis(100);
            let config = Config::default();
            let script = make_temp_script("#!/bin/sh\nsleep 2\n");
            let spec = make_spec(script, false);

            let result = manager.ensure_started(spec, &config);
            assert!(result.is_err());
            assert!(matches!(manager.state, SidecarState::SubprocessOnly { .. }));
        }

        #[test]
        fn line_reader_handles_partial_json_frames() {
            let dir = TempDir::new().unwrap();
            let socket = dir.path().join("partial.sock");
            let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
            let handle = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf).unwrap();
                stream.write_all(br#"{"ok":true,"request_id":"x""#).unwrap();
                thread::sleep(Duration::from_millis(10));
                stream
                    .write_all(br#","text":"ok","elapsed_ms":1,"word_timestamps":[{"word":"ok","start":0.0,"end":0.1,"confidence":1.0}]}"#)
                    .unwrap();
                stream.write_all(b"\n").unwrap();
            });

            let request = SidecarRequest {
                request_id: "x".into(),
                audio_path: "/tmp/test.wav".into(),
                decoder: "tdt".into(),
                timestamps: true,
                use_vad: false,
                beam_width: 8,
                lm_path: String::new(),
                lm_weight: 0.5,
                boost_phrases: Vec::new(),
                boost_score: 0.0,
            };
            let line = request_sidecar(
                &socket,
                &request,
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .unwrap();
            handle.join().unwrap();
            assert!(line.contains("\"request_id\":\"x\""));
            assert!(line.contains("\"word_timestamps\""));
        }
    }
}

#[cfg(all(feature = "parakeet", unix))]
pub use imp::*;

#[cfg(not(all(feature = "parakeet", unix)))]
mod imp_stub {
    use crate::config::Config;
    use std::path::Path;

    #[derive(Debug, Clone)]
    pub struct SidecarLaunchSpec;

    #[derive(Debug, Clone)]
    pub struct SidecarRequest;

    #[derive(Debug, Clone)]
    pub struct SidecarTranscriptResult;

    #[derive(Debug, Clone)]
    pub struct SidecarError {
        message: String,
    }

    impl std::fmt::Display for SidecarError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.message)
        }
    }

    impl std::error::Error for SidecarError {}

    pub fn build_launch_spec(
        _config: &Config,
        _model_path: &Path,
        _vocab_path: &Path,
        _vad_path: Option<&Path>,
    ) -> Result<SidecarLaunchSpec, SidecarError> {
        Err(SidecarError {
            message: "parakeet-sidecar: unavailable on this build".into(),
        })
    }

    pub fn resolve_server_binary(_parakeet_binary: &str) -> Option<std::path::PathBuf> {
        None
    }

    pub fn transcribe_via_global_sidecar(
        _config: &Config,
        _model_path: &Path,
        _vocab_path: &Path,
        _vad_path: Option<&Path>,
        _wav_path: &Path,
        _audio_duration_secs: f64,
    ) -> Result<SidecarTranscriptResult, SidecarError> {
        Err(SidecarError {
            message: "parakeet-sidecar: unavailable on this build".into(),
        })
    }

    pub fn warmup_global_sidecar(
        _config: &Config,
        _model_path: &Path,
        _vocab_path: &Path,
        _vad_path: Option<&Path>,
    ) -> Result<bool, SidecarError> {
        Err(SidecarError {
            message: "parakeet-sidecar: unavailable on this build".into(),
        })
    }

    pub fn shutdown_global_parakeet_sidecar() {}
}

#[cfg(not(all(feature = "parakeet", unix)))]
pub use imp_stub::*;
