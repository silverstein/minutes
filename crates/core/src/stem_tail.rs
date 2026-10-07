//! Incremental reader for a capture stem that is still being written.
//!
//! Native call capture (ScreenCaptureKit) writes microphone and system audio to
//! separate WAV stems as the call runs, but the recording path never started the
//! live-transcription sidecar, so live consumers received nothing during a
//! meeting even though the post-stop transcript was fine (#576).
//!
//! The sidecar is already decoupled: it consumes 16 kHz mono `f32` chunks over
//! an mpsc channel and does not care where they come from. This module supplies
//! them by tailing the growing stems, which avoids touching the Swift helper.
//!
//! Two properties of those files shape the implementation:
//!
//! - The header is not finalized until capture stops. The declared `data` size
//!   stays at its placeholder while samples accumulate (the same condition that
//!   made recovery reject healthy audio in #519), so the tailer measures the
//!   file on each poll instead of trusting the header.
//! - Reads land mid-frame. A poll can stop partway through a sample or a frame,
//!   so leftover bytes carry into the next poll rather than being dropped.

use crate::config::Config;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Sample rate the live sidecar consumes.
const TARGET_RATE: u32 = 16_000;

/// Sample encodings the native call helper can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    F32,
    I16,
}

impl Encoding {
    fn bytes_per_sample(self) -> usize {
        match self {
            Encoding::F32 => 4,
            Encoding::I16 => 2,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct StemFormat {
    channels: u16,
    sample_rate: u32,
    encoding: Encoding,
}

/// A growing stem, read forward from where the last poll stopped.
#[derive(Debug)]
pub(crate) struct StemTail {
    path: PathBuf,
    format: StemFormat,
    /// Absolute byte offset of the next unread audio byte.
    cursor: u64,
    /// Bytes of a frame that a previous poll could not complete.
    carry: Vec<u8>,
    /// Fractional read position for decimation, carried across polls so the
    /// resampled stream stays continuous instead of restarting each time.
    resample_pos: f64,
}

impl StemTail {
    /// Open a stem and position the cursor at the first audio byte.
    ///
    /// Fails while the header is too short to describe the format, which is
    /// normal in the first moments of a capture; the caller retries.
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
        let mut header = [0_u8; 4096];
        let read = file
            .read(&mut header)
            .map_err(|e| format!("read header {}: {e}", path.display()))?;
        let (format, data_offset) = parse_header(&header[..read])?;
        Ok(Self {
            path: path.to_path_buf(),
            format,
            cursor: data_offset,
            carry: Vec::new(),
            resample_pos: 0.0,
        })
    }

    /// Read whatever has been appended since the last poll, as 16 kHz mono.
    ///
    /// Returns an empty vector when the writer has not advanced. Errors are
    /// transient by nature here (the file is being written concurrently), so the
    /// caller should treat them as "nothing this round" rather than fatal.
    pub(crate) fn poll(&mut self) -> Result<Vec<f32>, String> {
        let mut file =
            File::open(&self.path).map_err(|e| format!("reopen {}: {e}", self.path.display()))?;
        let len = file
            .metadata()
            .map_err(|e| format!("stat {}: {e}", self.path.display()))?
            .len();
        if len <= self.cursor {
            return Ok(Vec::new());
        }

        let want = (len - self.cursor) as usize;
        let mut fresh = vec![0_u8; want];
        file.seek(SeekFrom::Start(self.cursor))
            .map_err(|e| format!("seek {}: {e}", self.path.display()))?;
        let got = file
            .read(&mut fresh)
            .map_err(|e| format!("read {}: {e}", self.path.display()))?;
        fresh.truncate(got);
        self.cursor += got as u64;

        if !self.carry.is_empty() {
            let mut joined = std::mem::take(&mut self.carry);
            joined.extend_from_slice(&fresh);
            fresh = joined;
        }

        let frame = self.format.channels as usize * self.format.encoding.bytes_per_sample();
        if frame == 0 {
            return Err("stem reports zero-width frames".into());
        }
        let usable = fresh.len() - (fresh.len() % frame);
        self.carry = fresh[usable..].to_vec();

        Ok(self.decode_to_mono_16k(&fresh[..usable]))
    }

    /// Decode whole frames, average channels to mono, and decimate to 16 kHz.
    fn decode_to_mono_16k(&mut self, bytes: &[u8]) -> Vec<f32> {
        let channels = self.format.channels as usize;
        let width = self.format.encoding.bytes_per_sample();
        let frames = bytes.len() / (channels * width);
        let ratio = self.format.sample_rate as f64 / TARGET_RATE as f64;

        let mut out = Vec::with_capacity(((frames as f64) / ratio).ceil() as usize + 1);
        for frame in 0..frames {
            // Average the channels rather than taking the first: the system stem
            // can carry content on one side only, and dropping a channel would
            // silence it.
            let mut sum = 0.0_f32;
            for ch in 0..channels {
                let at = (frame * channels + ch) * width;
                sum += match self.format.encoding {
                    Encoding::F32 => {
                        f32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
                    }
                    Encoding::I16 => {
                        i16::from_le_bytes([bytes[at], bytes[at + 1]]) as f32 / 32768.0
                    }
                };
            }
            let mono = sum / channels as f32;

            // Nearest-source decimation, matching the capture path's approach.
            // `resample_pos` persists across polls so chunk boundaries do not
            // restart the phase and introduce a click.
            if self.resample_pos <= frame as f64 {
                out.push(mono);
                self.resample_pos += ratio;
            }
        }
        // Rebase so the position stays relative to the next chunk.
        self.resample_pos = (self.resample_pos - frames as f64).max(0.0);
        out
    }
}

/// Locate the `fmt ` and `data` chunks in a RIFF/WAVE header.
///
/// Returns the format and the absolute offset of the first audio byte. The
/// declared `data` size is deliberately ignored: it is a placeholder until the
/// writer finalizes the file, and the caller measures the real length instead.
fn parse_header(bytes: &[u8]) -> Result<(StemFormat, u64), String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE stem".into());
    }

    let mut at = 12_usize;
    let mut format: Option<StemFormat> = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]])
            as usize;
        let body = at + 8;

        if id == b"fmt " {
            if body + 16 > bytes.len() {
                return Err("fmt chunk truncated".into());
            }
            let tag = u16::from_le_bytes([bytes[body], bytes[body + 1]]);
            let channels = u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]);
            let sample_rate = u32::from_le_bytes([
                bytes[body + 4],
                bytes[body + 5],
                bytes[body + 6],
                bytes[body + 7],
            ]);
            let bits = u16::from_le_bytes([bytes[body + 14], bytes[body + 15]]);
            // 0xFFFE is WAVE_FORMAT_EXTENSIBLE; the helper writes float there,
            // and bit depth disambiguates the rest.
            let encoding = match (tag, bits) {
                (3, 32) | (0xFFFE, 32) => Encoding::F32,
                (1, 16) | (0xFFFE, 16) => Encoding::I16,
                _ => return Err(format!("unsupported stem format (tag {tag}, {bits} bits)")),
            };
            if channels == 0 || channels > 32 || sample_rate == 0 {
                return Err("implausible stem format".into());
            }
            format = Some(StemFormat {
                channels,
                sample_rate,
                encoding,
            });
        } else if id == b"data" {
            let format = format.ok_or_else(|| "data chunk before fmt".to_string())?;
            return Ok((format, body as u64));
        }

        // Chunks are word-aligned.
        at = body + size + (size & 1);
    }
    Err("no data chunk in header window".into())
}

/// How often to check the stems for newly written audio.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// How long to keep waiting for a stem's header to appear before giving up.
///
/// The helper creates the files and writes headers within moments of starting,
/// so this only has to outlast process startup.
const HEADER_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Give the sibling stem a brief chance to publish its header after the first
/// source becomes usable. The native helper normally creates both together,
/// but their first writes can race. After this grace period, start with the
/// usable source instead of sacrificing the entire live transcript.
const SIBLING_STEM_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// If one opened stem stops producing frames while its sibling advances, do
/// not let the stalled source block the healthy source forever. This also
/// covers a header-only microphone stem when macOS grants capture but delivers
/// no microphone frames.
const STEM_STALL_GRACE_SECS: u64 = 2;
const STEM_STALL_GRACE: std::time::Duration = std::time::Duration::from_secs(STEM_STALL_GRACE_SECS);

/// Require the healthy sibling to have advanced through the full stall grace
/// before excluding the other source. A single late frame must not turn a
/// momentary two-source pause into a permanent one-source transcript.
const STEM_STALL_MIN_ADVANCE_SAMPLES: usize = TARGET_RATE as usize * STEM_STALL_GRACE_SECS as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StalledSource {
    Voice,
    System,
}

/// Feed the live-transcription sidecar from native call-capture stems.
///
/// Native capture writes audio to WAV stems rather than handing samples to the
/// recording path, so the sidecar had no source and live consumers received
/// nothing during a call (#576). This tails those stems and forwards the audio,
/// which needs no change to the Swift helper.
///
/// Returns the sidecar's join handle when live transcription is available, and
/// `None` when the build lacks the whisper/streaming features or the sidecar
/// could not start.
///
/// **Failure-isolated by construction.** Everything here runs on its own thread
/// and only ever reads the stems. A stem that never appears, a malformed
/// header, or a read error ends this thread quietly; recording and stem
/// preservation continue regardless. That is the capture-reliability invariant:
/// an optional consumer must never be able to degrade capture.
pub fn spawn_live_transcription_from_stems(
    voice_stem: PathBuf,
    system_stem: Option<PathBuf>,
    config: &Config,
    stop_flag: Arc<AtomicBool>,
    partial_publisher: Option<crate::live_partials::LivePartialPublisher>,
) -> Option<std::thread::JoinHandle<()>> {
    let (live_tx, sidecar_handle) =
        crate::capture::start_live_sidecar(config, &stop_flag, partial_publisher);
    let live_tx = live_tx?;

    let feeder = std::thread::Builder::new()
        .name("stem-tail-feeder".into())
        .spawn(move || {
            feed_from_stems(voice_stem, system_stem, &live_tx, &stop_flag);
            // Dropping the sender closes the channel so the sidecar finalizes
            // rather than waiting on a producer that is gone.
            drop(live_tx);
        });

    match feeder {
        Ok(_) => sidecar_handle,
        Err(error) => {
            tracing::warn!(%error, "could not start stem tail feeder; live transcript unavailable");
            sidecar_handle
        }
    }
}

/// Open whichever stems are usable, then forward audio until asked to stop.
fn feed_from_stems(
    voice_stem: PathBuf,
    system_stem: Option<PathBuf>,
    live_tx: &std::sync::mpsc::SyncSender<Vec<f32>>,
    stop_flag: &Arc<AtomicBool>,
) {
    let Some((mut voice, mut system)) = wait_for_stems(
        &voice_stem,
        system_stem.as_deref(),
        stop_flag,
        SIBLING_STEM_GRACE,
    ) else {
        return;
    };
    let mut has_voice = voice.is_some();
    let mut has_system = system.is_some();

    if !has_voice {
        tracing::warn!(
            stem = %voice_stem.display(),
            "voice stem unavailable; live transcript is using system audio only"
        );
    }
    if let Some(system_stem) = system_stem.as_deref().filter(|_| !has_system) {
        tracing::warn!(
            stem = %system_stem.display(),
            "system stem unavailable; live transcript is using voice audio only"
        );
    }

    // Per-stem residue: the stems advance independently, so each round mixes the
    // overlapping prefix and keeps the rest for the next one.
    let mut voice_pending: Vec<f32> = Vec::new();
    let mut system_pending: Vec<f32> = Vec::new();
    let started_at = std::time::Instant::now();
    let mut voice_last_progress = started_at;
    let mut system_last_progress = started_at;

    while !stop_flag.load(Ordering::Relaxed) {
        if let Some(voice) = voice.as_mut() {
            match poll_source(
                voice,
                &mut has_voice,
                &mut voice_pending,
                &mut voice_last_progress,
            ) {
                Ok(true) => {
                    tracing::info!(
                        "voice stem resumed producing frames; re-attaching it to the live transcript mix"
                    );
                }
                Ok(false) => {}
                Err(error) => tracing::debug!(%error, "voice stem poll failed; continuing"),
            }
        }
        if let Some(system) = system.as_mut() {
            match poll_source(
                system,
                &mut has_system,
                &mut system_pending,
                &mut system_last_progress,
            ) {
                Ok(true) => {
                    tracing::info!(
                        "system stem resumed producing frames; re-attaching it to the live transcript mix"
                    );
                }
                Ok(false) => {}
                Err(error) => tracing::debug!(%error, "system stem poll failed; continuing"),
            }
        }

        let now = std::time::Instant::now();
        match stalled_source_to_drop(
            has_voice,
            has_system,
            voice_pending.len(),
            system_pending.len(),
            now.duration_since(voice_last_progress),
            now.duration_since(system_last_progress),
        ) {
            Some(StalledSource::Voice) => {
                has_voice = false;
                voice_pending.clear();
                tracing::warn!(
                    "voice stem stopped producing frames; live transcript is continuing with system audio only and will re-attach voice if frames resume"
                );
            }
            Some(StalledSource::System) => {
                has_system = false;
                system_pending.clear();
                tracing::warn!(
                    "system stem stopped producing frames; live transcript is continuing with voice audio only and will re-attach system audio if frames resume"
                );
            }
            None => {}
        }

        let chunk = take_audio(
            &mut voice_pending,
            &mut system_pending,
            has_voice,
            has_system,
        );
        if !chunk.is_empty() && live_tx.send(chunk).is_err() {
            // The sidecar is gone; nothing left to feed.
            return;
        }

        std::thread::sleep(POLL_INTERVAL);
    }

    // Final sweep so the tail of the call is not lost to the stop race.
    if let Some(voice) = voice.as_mut() {
        if let Ok(samples) = voice.poll() {
            voice_pending.extend_from_slice(&samples);
        }
    }
    if let Some(system) = system.as_mut() {
        if let Ok(samples) = system.poll() {
            system_pending.extend_from_slice(&samples);
        }
    }
    let mut tail = take_audio(
        &mut voice_pending,
        &mut system_pending,
        has_voice,
        has_system,
    );
    // At stop there will be no later sibling frames to align with, so preserve
    // the longer stem's remaining suffix instead of silently discarding it.
    // One of these buffers is empty after `take_audio` mixes the overlap.
    tail.extend(std::mem::take(&mut voice_pending));
    tail.extend(std::mem::take(&mut system_pending));
    if !tail.is_empty() {
        let _ = live_tx.send(tail);
    }
}

/// Return audio for the selected source plan.
///
/// A single usable stem passes through untouched. With both stems, mix only
/// their overlapping prefix and buffer any surplus so the sources stay
/// aligned. Sums are clamped rather than scaled so a quiet side is not
/// attenuated by a loud one.
fn take_audio(
    voice: &mut Vec<f32>,
    system: &mut Vec<f32>,
    has_voice: bool,
    has_system: bool,
) -> Vec<f32> {
    match (has_voice, has_system) {
        (true, false) => return std::mem::take(voice),
        (false, true) => return std::mem::take(system),
        (false, false) => return Vec::new(),
        (true, true) => {}
    }
    let n = voice.len().min(system.len());
    if n == 0 {
        return Vec::new();
    }
    let mixed: Vec<f32> = voice
        .drain(..n)
        .zip(system.drain(..n))
        .map(|(a, b)| (a + b).clamp(-1.0, 1.0))
        .collect();
    mixed
}

fn stalled_source_to_drop(
    has_voice: bool,
    has_system: bool,
    voice_pending: usize,
    system_pending: usize,
    voice_idle: std::time::Duration,
    system_idle: std::time::Duration,
) -> Option<StalledSource> {
    if !(has_voice && has_system) {
        return None;
    }
    if voice_pending == 0
        && system_pending >= STEM_STALL_MIN_ADVANCE_SAMPLES
        && voice_idle >= STEM_STALL_GRACE
    {
        return Some(StalledSource::Voice);
    }
    if system_pending == 0
        && voice_pending >= STEM_STALL_MIN_ADVANCE_SAMPLES
        && system_idle >= STEM_STALL_GRACE
    {
        return Some(StalledSource::System);
    }
    None
}

/// Re-enable a source in the live mix as soon as its still-open tail produces
/// frames again. The tail itself is deliberately retained while excluded so a
/// transient device or route interruption cannot silence that source for the
/// rest of the call.
fn reattach_resumed_source(has_source: &mut bool) -> bool {
    if !*has_source {
        *has_source = true;
        return true;
    }
    false
}

fn poll_source(
    tail: &mut StemTail,
    has_source: &mut bool,
    pending: &mut Vec<f32>,
    last_progress: &mut std::time::Instant,
) -> Result<bool, String> {
    let samples = tail.poll()?;
    if samples.is_empty() {
        return Ok(false);
    }
    *last_progress = std::time::Instant::now();
    pending.extend_from_slice(&samples);
    Ok(reattach_resumed_source(has_source))
}

/// Wait for at least one stem to exist and carry a parseable header.
///
/// Voice and system are probed in parallel. Once either is ready, the sibling
/// gets a short grace period to avoid dropping one side because of a startup
/// race. A missing voice stem must not suppress healthy system audio.
fn wait_for_stems(
    voice_path: &Path,
    system_path: Option<&Path>,
    stop_flag: &Arc<AtomicBool>,
    sibling_grace: std::time::Duration,
) -> Option<(Option<StemTail>, Option<StemTail>)> {
    let deadline = std::time::Instant::now() + HEADER_WAIT;
    let mut voice = None;
    let mut system = None;
    let mut first_ready_at = None;

    while std::time::Instant::now() < deadline {
        if stop_flag.load(Ordering::Relaxed) {
            return None;
        }

        if voice.is_none() {
            voice = StemTail::open(voice_path).ok();
        }
        if system.is_none() {
            system = system_path.and_then(|path| StemTail::open(path).ok());
        }

        let any_ready = voice.is_some() || system.is_some();
        let all_expected_ready = voice.is_some() && (system_path.is_none() || system.is_some());
        if all_expected_ready {
            return Some((voice, system));
        }
        if any_ready {
            let ready_at = first_ready_at.get_or_insert_with(std::time::Instant::now);
            if ready_at.elapsed() >= sibling_grace {
                return Some((voice, system));
            }
        }

        std::thread::sleep(POLL_INTERVAL);
    }
    if voice.is_some() || system.is_some() {
        return Some((voice, system));
    }
    tracing::warn!(
        voice_stem = %voice_path.display(),
        system_stem = system_path.map(|path| path.display().to_string()),
        "no usable stem header appeared; live transcript will not run for this capture"
    );
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a stem whose header is unfinalized, matching what the native
    /// helper leaves on disk while a call is in progress.
    fn write_growing_stem(path: &Path, channels: u16, rate: u32, encoding: Encoding) {
        let mut f = File::create(path).unwrap();
        let (tag, bits) = match encoding {
            Encoding::F32 => (3_u16, 32_u16),
            Encoding::I16 => (1_u16, 16_u16),
        };
        let block_align = channels * (bits / 8);
        let byte_rate = rate * block_align as u32;

        f.write_all(b"RIFF").unwrap();
        f.write_all(&4088_u32.to_le_bytes()).unwrap(); // placeholder
        f.write_all(b"WAVE").unwrap();
        f.write_all(b"fmt ").unwrap();
        f.write_all(&16_u32.to_le_bytes()).unwrap();
        f.write_all(&tag.to_le_bytes()).unwrap();
        f.write_all(&channels.to_le_bytes()).unwrap();
        f.write_all(&rate.to_le_bytes()).unwrap();
        f.write_all(&byte_rate.to_le_bytes()).unwrap();
        f.write_all(&block_align.to_le_bytes()).unwrap();
        f.write_all(&bits.to_le_bytes()).unwrap();
        f.write_all(b"data").unwrap();
        f.write_all(&0_u32.to_le_bytes()).unwrap(); // placeholder, never updated
    }

    fn append_f32(path: &Path, samples: &[f32]) {
        let mut f = File::options().append(true).open(path).unwrap();
        for s in samples {
            f.write_all(&s.to_le_bytes()).unwrap();
        }
    }

    #[test]
    fn reads_only_what_was_appended_since_the_last_poll() {
        let dir = tempfile::tempdir().unwrap();
        let stem = dir.path().join("voice.wav");
        write_growing_stem(&stem, 1, 16_000, Encoding::F32);

        let mut tail = StemTail::open(&stem).unwrap();
        assert!(tail.poll().unwrap().is_empty(), "nothing written yet");

        append_f32(&stem, &[0.1, 0.2, 0.3, 0.4]);
        assert_eq!(tail.poll().unwrap().len(), 4);

        // A second poll with no new bytes must not replay the same audio.
        assert!(tail.poll().unwrap().is_empty());

        append_f32(&stem, &[0.5, 0.6]);
        assert_eq!(tail.poll().unwrap().len(), 2);
    }

    #[test]
    fn carries_a_partial_frame_across_polls() {
        let dir = tempfile::tempdir().unwrap();
        let stem = dir.path().join("voice.wav");
        write_growing_stem(&stem, 1, 16_000, Encoding::F32);
        let mut tail = StemTail::open(&stem).unwrap();

        // Three bytes: less than one 4-byte f32 frame.
        {
            let mut f = File::options().append(true).open(&stem).unwrap();
            f.write_all(&[1, 2, 3]).unwrap();
        }
        assert!(
            tail.poll().unwrap().is_empty(),
            "a partial frame must not be decoded"
        );

        // The fourth byte completes it.
        {
            let mut f = File::options().append(true).open(&stem).unwrap();
            f.write_all(&[4]).unwrap();
        }
        assert_eq!(
            tail.poll().unwrap().len(),
            1,
            "the carried bytes complete one frame"
        );
    }

    #[test]
    fn downmixes_channels_rather_than_dropping_one() {
        let dir = tempfile::tempdir().unwrap();
        let stem = dir.path().join("system.wav");
        write_growing_stem(&stem, 2, 16_000, Encoding::F32);
        let mut tail = StemTail::open(&stem).unwrap();

        // Content only on the right channel: taking channel 0 would silence it.
        append_f32(&stem, &[0.0, 1.0]);
        let out = tail.poll().unwrap();
        assert_eq!(out.len(), 1);
        assert!(
            (out[0] - 0.5).abs() < 1e-6,
            "expected the average, got {out:?}"
        );
    }

    #[test]
    fn decimates_to_16k_and_keeps_phase_across_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let stem = dir.path().join("voice.wav");
        write_growing_stem(&stem, 1, 48_000, Encoding::F32);
        let mut tail = StemTail::open(&stem).unwrap();

        // 48 kHz in, 16 kHz out: expect roughly a third of the frames.
        append_f32(&stem, &[0.25_f32; 48]);
        let first = tail.poll().unwrap().len();
        append_f32(&stem, &[0.25_f32; 48]);
        let second = tail.poll().unwrap().len();

        assert!((15..=17).contains(&first), "first chunk: {first}");
        assert!((15..=17).contains(&second), "second chunk: {second}");
    }

    #[test]
    fn decodes_i16_stems() {
        let dir = tempfile::tempdir().unwrap();
        let stem = dir.path().join("voice.wav");
        write_growing_stem(&stem, 1, 16_000, Encoding::I16);
        let mut tail = StemTail::open(&stem).unwrap();

        let mut f = File::options().append(true).open(&stem).unwrap();
        f.write_all(&16384_i16.to_le_bytes()).unwrap();
        drop(f);

        let out = tail.poll().unwrap();
        assert_eq!(out.len(), 1);
        assert!((out[0] - 0.5).abs() < 1e-3, "got {out:?}");
    }

    #[test]
    fn mixing_waits_for_both_stems_and_buffers_the_surplus() {
        // The stems advance independently, so a round must mix only what both
        // have and keep the rest rather than emitting unaligned audio.
        let mut voice = vec![0.1, 0.2, 0.3];
        let mut system = vec![0.4];

        let mixed = take_audio(&mut voice, &mut system, true, true);
        assert_eq!(mixed.len(), 1, "only the overlapping prefix is emitted");
        assert!((mixed[0] - 0.5).abs() < 1e-6);
        assert_eq!(voice.len(), 2, "unmatched voice audio stays buffered");
        assert!(system.is_empty());
    }

    #[test]
    fn mixing_passes_voice_through_when_there_is_no_system_stem() {
        let mut voice = vec![0.1, 0.2];
        let mut system = Vec::new();
        let out = take_audio(&mut voice, &mut system, true, false);
        assert_eq!(out, vec![0.1, 0.2]);
        assert!(voice.is_empty(), "everything is consumed");
    }

    #[test]
    fn mixing_passes_system_through_when_voice_is_unavailable() {
        let mut voice = Vec::new();
        let mut system = vec![0.3, 0.4];
        let out = take_audio(&mut voice, &mut system, false, true);
        assert_eq!(out, vec![0.3, 0.4]);
        assert!(system.is_empty(), "everything is consumed");
    }

    #[test]
    fn mixing_clamps_instead_of_wrapping() {
        // Two loud sides must not produce out-of-range samples.
        let mut voice = vec![0.9];
        let mut system = vec![0.9];
        let out = take_audio(&mut voice, &mut system, true, true);
        assert_eq!(out, vec![1.0]);
    }

    #[test]
    fn final_mix_can_preserve_the_longer_stem_suffix() {
        let mut voice = vec![0.1, 0.2, 0.3];
        let mut system = vec![0.4];

        let mut tail = take_audio(&mut voice, &mut system, true, true);
        tail.extend(std::mem::take(&mut voice));
        tail.extend(std::mem::take(&mut system));

        assert_eq!(tail, vec![0.5, 0.2, 0.3]);
    }

    #[test]
    fn header_only_voice_does_not_block_advancing_system_audio() {
        assert_eq!(
            stalled_source_to_drop(
                true,
                true,
                0,
                STEM_STALL_MIN_ADVANCE_SAMPLES,
                STEM_STALL_GRACE,
                std::time::Duration::ZERO,
            ),
            Some(StalledSource::Voice)
        );
    }

    #[test]
    fn header_only_system_does_not_block_advancing_voice_audio() {
        assert_eq!(
            stalled_source_to_drop(
                true,
                true,
                STEM_STALL_MIN_ADVANCE_SAMPLES,
                0,
                std::time::Duration::ZERO,
                STEM_STALL_GRACE,
            ),
            Some(StalledSource::System)
        );
    }

    #[test]
    fn sibling_startup_grace_prevents_premature_source_drop() {
        assert_eq!(
            stalled_source_to_drop(
                true,
                true,
                0,
                STEM_STALL_MIN_ADVANCE_SAMPLES,
                STEM_STALL_GRACE - std::time::Duration::from_millis(1),
                std::time::Duration::ZERO,
            ),
            None
        );
    }

    #[test]
    fn less_than_a_grace_window_of_sibling_audio_does_not_drop_a_source() {
        assert_eq!(
            stalled_source_to_drop(
                true,
                true,
                0,
                STEM_STALL_MIN_ADVANCE_SAMPLES - 1,
                STEM_STALL_GRACE,
                std::time::Duration::ZERO,
            ),
            None
        );
    }

    #[test]
    fn stalled_voice_rejoins_the_mix_when_frames_resume() {
        let dir = tempfile::tempdir().unwrap();
        let voice_path = dir.path().join("voice.wav");
        write_growing_stem(&voice_path, 1, TARGET_RATE, Encoding::F32);
        let mut voice_tail = StemTail::open(&voice_path).unwrap();
        let mut has_voice = true;
        let has_system = true;
        let mut voice_pending = Vec::new();
        let mut system_pending = vec![0.4; STEM_STALL_MIN_ADVANCE_SAMPLES];
        let mut voice_last_progress = std::time::Instant::now();

        assert_eq!(
            stalled_source_to_drop(
                has_voice,
                has_system,
                voice_pending.len(),
                system_pending.len(),
                STEM_STALL_GRACE,
                std::time::Duration::ZERO,
            ),
            Some(StalledSource::Voice)
        );
        has_voice = false;
        voice_pending.clear();

        let system_only = take_audio(
            &mut voice_pending,
            &mut system_pending,
            has_voice,
            has_system,
        );
        assert_eq!(system_only.len(), STEM_STALL_MIN_ADVANCE_SAMPLES);

        append_f32(&voice_path, &[0.25]);
        system_pending.push(0.5);
        assert!(
            poll_source(
                &mut voice_tail,
                &mut has_voice,
                &mut voice_pending,
                &mut voice_last_progress,
            )
            .unwrap(),
            "the retained tail must report that voice re-attached"
        );
        assert!(has_voice, "voice must participate again in the same round");

        let rejoined = take_audio(
            &mut voice_pending,
            &mut system_pending,
            has_voice,
            has_system,
        );
        assert_eq!(rejoined, vec![0.75]);
    }

    #[test]
    fn waiting_for_stems_accepts_system_only_audio() {
        let dir = tempfile::tempdir().unwrap();
        let voice = dir.path().join("missing-voice.wav");
        let system = dir.path().join("system.wav");
        write_growing_stem(&system, 2, 48_000, Encoding::F32);
        let stop = Arc::new(AtomicBool::new(false));

        let start = std::time::Instant::now();
        let (voice_tail, system_tail) =
            wait_for_stems(&voice, Some(&system), &stop, std::time::Duration::ZERO)
                .expect("the healthy system stem should be sufficient");

        assert!(voice_tail.is_none());
        assert!(system_tail.is_some());
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "system-only startup should not wait for the missing voice stem"
        );
    }

    #[test]
    fn waiting_for_stems_gives_up_when_asked_to_stop() {
        // Failure isolation: a capture that stops before the helper writes a
        // header must not leave this thread parked for the full timeout.
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("never-created.wav");
        let stop = Arc::new(AtomicBool::new(true));

        let start = std::time::Instant::now();
        let result = wait_for_stems(&missing, None, &stop, std::time::Duration::ZERO);
        assert!(result.is_none());
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "should observe the stop flag immediately, took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn rejects_a_header_that_is_not_a_wav() {
        let dir = tempfile::tempdir().unwrap();
        let stem = dir.path().join("bogus.wav");
        std::fs::write(&stem, b"not a wav at all").unwrap();
        assert!(StemTail::open(&stem).is_err());
    }
}
