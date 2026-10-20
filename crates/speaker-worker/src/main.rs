//! Isolated CPU inference only. Never captures, downloads, enrolls or writes audio.
#[allow(dead_code)]
#[path = "../../core/src/diarize_full_protocol.rs"]
mod protocol;
mod stage_diagnostics;
#[cfg(windows)]
mod windows_handles;
use protocol::*;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;
use zeroize::Zeroizing;

struct CanonicalAudio {
    samples: Zeroizing<Vec<f32>>,
    sha256: String,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("full speaker worker: {error}");
        std::process::exit(1);
    }
}

fn pipeline_failure_class(error: &speakrs::PipelineError) -> &'static str {
    use speakrs::PipelineError as E;
    match error {
        E::ModelLoad(_) => "model_load",
        E::Inference(_) => "inference",
        E::UnsupportedExecutionMode(_) => "unsupported_mode",
        E::Segmentation(_) => "segmentation",
        E::Powerset(_) => "powerset",
        E::Plda(_) => "plda",
        E::Reconstruct(_) => "reconstruct",
        E::Queue(_) => "queue",
        E::Invariant(_) => "invariant",
        E::WorkerPanic { .. } => "worker_panic",
        E::WorkerSpawn { .. } => "worker_spawn",
        E::Backend { .. } => "backend",
        E::Other(_) => "other",
        _ => "unclassified",
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Fixed markers only: never send model paths, audio or arbitrary errors to
    // the host's diagnostic progress channel.
    eprintln!("MINUTES_SPEAKER_PHASE handles_started");
    #[cfg(windows)]
    windows_handles::close_inherited_handles()?;
    eprintln!("MINUTES_SPEAKER_PHASE handles_closed");
    if std::env::var("MINUTES_FULL_SPEAKER_WORKER_V1").as_deref() != Ok("1") {
        return Err("launch through the Minutes supervisor".into());
    }
    install_limits()?;
    #[cfg(unix)]
    let parent: u32 = std::env::var("MINUTES_FULL_SPEAKER_PARENT_PID")?.parse()?;
    #[cfg(unix)]
    if parent <= 1 || unsafe { libc::getppid() } as u32 != parent {
        return Err("supervising parent is unavailable".into());
    }
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(900);
        loop {
            #[cfg(unix)]
            if unsafe { libc::getppid() } as u32 != parent {
                std::process::exit(70);
            }
            if std::time::Instant::now() >= deadline {
                std::process::exit(70);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    let mut args = std::env::args_os().skip(1);
    let models = args
        .next()
        .map(PathBuf::from)
        .ok_or("models directory required")?;
    if args.next().is_some() {
        return Err("unexpected arguments".into());
    }
    // Hash the private copy that inference will reopen, rather than trusting
    // a model pathname that can change between verification and loading.
    let snapshot = tempfile::tempdir()?;
    for (name, _) in MODEL_FILES {
        let mut input = std::fs::File::open(models.join(name))?.take(40 * 1024 * 1024 + 1);
        let mut output = std::fs::File::create(snapshot.path().join(name))?;
        std::io::copy(&mut input, &mut output)?;
    }
    verify_models(snapshot.path())?;
    eprintln!("MINUTES_SPEAKER_PHASE awaiting_pcm");
    let CanonicalAudio {
        samples,
        sha256: audio_sha256,
    } = read_pcm(std::io::stdin().lock(), MAX_SAMPLES)?;
    eprintln!("MINUTES_SPEAKER_PHASE pcm_loaded");
    let sample_count = samples.len();
    let mut pipeline =
        speakrs::OwnedDiarizationPipeline::from_dir(snapshot.path(), speakrs::ExecutionMode::Cpu)?;
    eprintln!("MINUTES_SPEAKER_PHASE pipeline_ready");
    let step = pipeline.segmentation_step();
    if !step.is_finite()
        || step < 1.0
        || ((samples.len() as f64 / 16_000.0 / step).ceil() + 10.0) * 3.0 > 12_000.0
    {
        return Err("full speaker observation budget exceeded".into());
    }
    let inference_start = stage_diagnostics::install()?;
    let result = pipeline.run(&samples).inspect_err(|error| {
        eprintln!("MINUTES_SPEAKER_FAILURE {}", pipeline_failure_class(error));
    })?;
    stage_diagnostics::finished(inference_start);
    eprintln!("MINUTES_SPEAKER_PHASE inference_complete");
    if result.segments.len() > MAX_SEGMENTS {
        return Err("segment budget exceeded".into());
    }
    let duration = sample_count as f64 / 16_000.0;
    let mut segments = Vec::new();
    for segment in result.segments {
        if let Some(segment) =
            canonical_segment(&segment.speaker, segment.start, segment.end, duration)?
        {
            segments.push(segment);
        }
    }
    let output = serde_json::to_vec(&Response {
        protocol: PROTOCOL,
        engine: ENGINE.into(),
        model_revision: MODEL_REVISION.into(),
        sample_count,
        audio_sha256,
        segments,
    })?;
    if output.len() as u64 > OUTPUT_LIMIT {
        return Err("response byte budget exceeded".into());
    }
    std::io::stdout().lock().write_all(&output)?;
    Ok(())
}

fn read_pcm(
    mut input: impl Read,
    max_samples: usize,
) -> Result<CanonicalAudio, Box<dyn std::error::Error>> {
    let mut samples = Zeroizing::new(Vec::new());
    let mut bytes = Zeroizing::new([0_u8; 64 * 1024]);
    let mut carry = 0;
    let mut hasher = Sha256::new();
    loop {
        let read = input.read(&mut bytes[carry..])?;
        if read == 0 {
            if carry != 0 {
                return Err("truncated PCM sample".into());
            }
            break;
        }
        let available = carry + read;
        let complete = available - available % 4;
        if complete / 4 > max_samples - samples.len() {
            return Err("one-hour audio budget exceeded".into());
        }
        for sample in bytes[..complete].chunks_exact(4) {
            let value = f32::from_le_bytes(sample.try_into()?);
            if !value.is_finite() {
                return Err("non-finite PCM input".into());
            }
            samples.push(value);
        }
        hasher.update(&bytes[..complete]);
        bytes.copy_within(complete..available, 0);
        carry = available - complete;
    }
    if samples.is_empty() {
        return Err("empty PCM input".into());
    }
    Ok(CanonicalAudio {
        samples,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

fn canonical_segment(
    speaker: &str,
    start: f64,
    end: f64,
    duration: f64,
) -> Result<Option<Segment>, Box<dyn std::error::Error>> {
    let index: u8 = speaker
        .strip_prefix("SPEAKER_")
        .ok_or("unexpected native speaker label")?
        .parse()?;
    if index >= 64 || speaker != format!("SPEAKER_{index:02}") {
        return Err("unexpected native speaker label or speaker budget exceeded".into());
    }
    // Validate before clamping: f64::min would otherwise hide a NaN end time.
    if !start.is_finite() || !end.is_finite() || start < 0.0 || end <= start {
        return Err("invalid native speaker activity".into());
    }
    let end = end.min(duration);
    Ok((start < end).then(|| Segment {
        speaker: format!("SPEAKER_{}", index + 1),
        start,
        end,
    }))
}

#[cfg(unix)]
fn install_limits() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    let limit = {
        use mach2::kern_return::KERN_SUCCESS;
        use mach2::task::task_info;
        use mach2::task_info::{
            task_basic_info_64, task_info_t, TASK_BASIC_INFO_64, TASK_BASIC_INFO_64_COUNT,
        };
        let mut info = task_basic_info_64::default();
        let mut count = TASK_BASIC_INFO_64_COUNT;
        let status = unsafe {
            task_info(
                mach2::traps::mach_task_self(),
                TASK_BASIC_INFO_64,
                (&mut info as *mut task_basic_info_64).cast::<libc::c_int>() as task_info_t,
                &mut count,
            )
        };
        if status != KERN_SUCCESS || count != TASK_BASIC_INFO_64_COUNT {
            return Err("cannot measure worker virtual memory".into());
        }
        info.virtual_size
            .checked_add(MEMORY_BYTES)
            .ok_or("memory budget overflow")?
    };
    #[cfg(not(target_os = "macos"))]
    let limit = MEMORY_BYTES;
    let memory = libc::rlimit {
        rlim_cur: limit as libc::rlim_t,
        rlim_max: limit as libc::rlim_t,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &memory) } != 0 {
        return Err("cannot install worker memory ceiling".into());
    }
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
    Ok(())
}

#[cfg(windows)]
fn install_limits() -> Result<(), Box<dyn std::error::Error>> {
    // Only the host can attach the suspended child before its first instruction.
    // Refuse standalone use rather than imply a Windows memory fence exists.
    use windows_sys::Win32::System::JobObjects::{
        IsProcessInJob, JobObjectExtendedLimitInformation, QueryInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_JOB_MEMORY,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut in_job = 0;
    if unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut in_job) } == 0
        || in_job == 0
    {
        return Err("Windows speaker worker requires a supervising Job Object".into());
    }
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        QueryInformationJobObject(
            std::ptr::null_mut(),
            JobObjectExtendedLimitInformation,
            (&mut info as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&info) as u32,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err("cannot inspect supervising Job Object".into());
    }
    let flags = JOB_OBJECT_LIMIT_PROCESS_MEMORY
        | JOB_OBJECT_LIMIT_JOB_MEMORY
        | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if info.BasicLimitInformation.LimitFlags & flags != flags
        || info.ProcessMemoryLimit != MEMORY_BYTES as usize
        || info.JobMemoryLimit != MEMORY_BYTES as usize
    {
        return Err("Windows speaker worker memory ceiling is missing".into());
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn install_limits() -> Result<(), Box<dyn std::error::Error>> {
    Err("unsupported worker platform".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn pcm_pipe_fragmentation_preserves_samples_and_digest() {
        use sha2::Digest;
        use std::io::Read;
        struct Fragmented<'a> {
            remaining: &'a [u8],
            fragment: usize,
        }
        impl Read for Fragmented<'_> {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                let count = output.len().min(self.fragment).min(self.remaining.len());
                output[..count].copy_from_slice(&self.remaining[..count]);
                self.remaining = &self.remaining[count..];
                Ok(count)
            }
        }
        let samples: Vec<f32> = (0..50_000).map(|i| (i % 17) as f32 / 17.0).collect();
        let bytes: Vec<u8> = samples.iter().flat_map(|f| f.to_le_bytes()).collect();
        let expected_digest = format!("{:x}", sha2::Sha256::digest(&bytes));
        for fragment in [1, 2, 3, 7, 65_535, 65_536] {
            let actual = super::read_pcm(
                Fragmented {
                    remaining: &bytes,
                    fragment,
                },
                samples.len(),
            )
            .unwrap();
            assert_eq!(*actual.samples, samples);
            assert_eq!(actual.sha256, expected_digest);
        }
    }

    #[test]
    fn pcm_pipe_rejects_truncation_nonfinite_and_over_budget_input() {
        for length in [0, 1, 2, 3, 5, 6, 7] {
            assert!(super::read_pcm(std::io::Cursor::new(vec![0; length]), 2).is_err());
        }
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(super::read_pcm(std::io::Cursor::new(invalid.to_le_bytes()), 2).is_err());
        }
        assert!(super::read_pcm(std::io::Cursor::new(vec![0; 12]), 2).is_err());
    }

    #[test]
    fn invalid_native_activity_cannot_be_hidden_by_padding_clamp() {
        for (start, end) in [
            (0.0, f64::NAN),
            (f64::NAN, 1.0),
            (0.0, f64::INFINITY),
            (-1.0, 1.0),
            (1.0, 1.0),
            (2.0, 1.0),
        ] {
            assert!(super::canonical_segment("SPEAKER_00", start, end, 2.0).is_err());
        }
        for label in ["Mat", "SPEAKER_0", "SPEAKER_64"] {
            assert!(super::canonical_segment(label, 0.0, 1.0, 2.0).is_err());
        }
        let padded = super::canonical_segment("SPEAKER_00", 1.0, 3.0, 2.0)
            .unwrap()
            .unwrap();
        assert_eq!(
            (padded.speaker.as_str(), padded.start, padded.end),
            ("SPEAKER_1", 1.0, 2.0)
        );
        assert!(super::canonical_segment("SPEAKER_00", 2.0, 3.0, 2.0)
            .unwrap()
            .is_none());
    }

    #[test]
    fn pipeline_failure_class_does_not_reflect_error_context_or_payload() {
        for (error, expected) in [
            (
                speakrs::PipelineError::Other("private/path or audio".into()),
                "other",
            ),
            (
                speakrs::PipelineError::Backend {
                    context: "private context",
                    message: "private payload".into(),
                },
                "backend",
            ),
            (
                speakrs::PipelineError::WorkerSpawn {
                    worker: "private worker",
                    source: std::io::Error::other("private detail"),
                },
                "worker_spawn",
            ),
        ] {
            let class = super::pipeline_failure_class(&error);
            assert_eq!(class, expected);
            assert!(super::FAILURE_CLASSES.contains(&class));
        }
    }

    #[cfg(unix)]
    #[test]
    fn memory_ceiling_refuses_an_over_budget_mapping() {
        const MARKER: &str = "MINUTES_SPEAKER_MEMORY_TEST_CHILD";
        if std::env::var_os(MARKER).is_some() {
            super::install_limits().unwrap();
            let bytes = (super::MEMORY_BYTES * 2) as usize;
            let mapping = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    bytes,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANON,
                    -1,
                    0,
                )
            };
            if mapping != libc::MAP_FAILED {
                unsafe {
                    libc::munmap(mapping, bytes);
                }
                panic!("worker memory ceiling allowed an over-budget mapping");
            }
            return;
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::memory_ceiling_refuses_an_over_budget_mapping",
                "--nocapture",
            ])
            .env(MARKER, "1")
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "memory ceiling child failed");
                break;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("memory ceiling child did not finish");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
