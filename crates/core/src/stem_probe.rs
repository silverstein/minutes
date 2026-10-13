use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

const RIFF_HEADER_LEN: u64 = 12;
const CHUNK_HEADER_LEN: u64 = 8;
const PCM_FORMAT: u16 = 1;
const IEEE_FLOAT_FORMAT: u16 = 3;
const EXTENSIBLE_FORMAT: u16 = 0xfffe;
const PCM_SUBFORMAT: [u8; 16] = [
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];
const FLOAT_SUBFORMAT: [u8; 16] = [
    0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];
const STEM_PROBE_RMS_FLOOR: f32 = 0.001;
const FORMAT_EVIDENCE_MIN_SAMPLES: u64 = 4_096;
const FORMAT_EVIDENCE_MIN_NONZERO: u64 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SampleEncoding {
    Unsigned8,
    Signed16,
    Signed24,
    Signed32,
    Float32,
}

impl SampleEncoding {
    fn bytes_per_sample(self) -> usize {
        match self {
            Self::Unsigned8 => 1,
            Self::Signed16 => 2,
            Self::Signed24 => 3,
            Self::Signed32 | Self::Float32 => 4,
        }
    }

    fn format_fields(self, channels: u16, sample_rate: u32) -> Option<FormatFields> {
        let (audio_format, bits_per_sample) = match self {
            Self::Unsigned8 => (PCM_FORMAT, 8),
            Self::Signed16 => (PCM_FORMAT, 16),
            Self::Signed24 => (PCM_FORMAT, 24),
            Self::Signed32 => (PCM_FORMAT, 32),
            Self::Float32 => (IEEE_FLOAT_FORMAT, 32),
        };
        let block_align = channels.checked_mul(bits_per_sample / 8)?;
        let byte_rate = sample_rate.checked_mul(u32::from(block_align))?;
        Some(FormatFields {
            audio_format,
            byte_rate,
            block_align,
            bits_per_sample,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FormatFields {
    audio_format: u16,
    byte_rate: u32,
    block_align: u16,
    bits_per_sample: u16,
}

#[derive(Debug, Clone, Copy)]
struct WavFormat {
    audio_format: u16,
    channels: u16,
    sample_rate: u32,
    byte_rate: u32,
    block_align: u16,
    bits_per_sample: u16,
}

impl WavFormat {
    fn encoding(self) -> Option<SampleEncoding> {
        match (self.audio_format, self.bits_per_sample) {
            (PCM_FORMAT, 8) => Some(SampleEncoding::Unsigned8),
            (PCM_FORMAT, 16) => Some(SampleEncoding::Signed16),
            (PCM_FORMAT, 24) => Some(SampleEncoding::Signed24),
            (PCM_FORMAT, 32) => Some(SampleEncoding::Signed32),
            (IEEE_FLOAT_FORMAT, 32) => Some(SampleEncoding::Float32),
            _ => None,
        }
    }

    fn is_sane(self) -> bool {
        if self.channels == 0 || self.channels > 32 || self.sample_rate == 0 {
            return false;
        }
        let Some(encoding) = self.encoding() else {
            return false;
        };
        let Some(expected) = encoding.format_fields(self.channels, self.sample_rate) else {
            return false;
        };
        self.byte_rate == expected.byte_rate && self.block_align == expected.block_align
    }
}

#[derive(Debug)]
struct WavLayout {
    file_len: u64,
    declared_riff_size: u32,
    fmt_offset: u64,
    format_container: FormatContainer,
    format: WavFormat,
    data_size_offset: u64,
    data_offset: u64,
    declared_data_size: u32,
    actual_data_size: u64,
    large_container: bool,
}

#[derive(Debug, Clone, Copy)]
enum FormatContainer {
    Classic,
    Extensible { subformat_offset: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StemSignal {
    Missing,
    Empty,
    Invalid,
    Silent,
    Usable,
}

#[derive(Debug)]
pub(crate) struct StemProbe {
    pub(crate) signal: StemSignal,
    pub(crate) repaired: bool,
    pub(crate) format_repaired: bool,
    detail: Option<String>,
}

impl StemProbe {
    pub(crate) fn is_usable(&self) -> bool {
        self.signal == StemSignal::Usable
    }

    pub(crate) fn description(&self) -> String {
        match (&self.signal, &self.detail) {
            (StemSignal::Missing, _) => "missing".to_string(),
            (StemSignal::Empty, _) => "empty".to_string(),
            (StemSignal::Invalid, Some(detail)) => format!("invalid ({detail})"),
            (StemSignal::Invalid, None) => "invalid".to_string(),
            (StemSignal::Silent, _) => "digitally silent".to_string(),
            (StemSignal::Usable, _) => "usable".to_string(),
        }
    }
}

#[derive(Debug)]
enum ParsedStem {
    Missing,
    Invalid(String),
    Wav(WavLayout),
}

/// Probe a native-call WAV stem without trusting its RIFF or data size fields.
///
/// When the RIFF structure and `fmt ` chunk are sane, the payload is always
/// measured from the data offset to EOF. Verified placeholder or mismatched
/// headers are repaired in place before this function returns. Only header
/// fields are changed; the audio payload is never rewritten.
pub(crate) fn probe_and_repair(path: &Path) -> Result<StemProbe, String> {
    let layout = match parse_wav(path)? {
        ParsedStem::Missing => {
            return Ok(StemProbe {
                signal: StemSignal::Missing,
                repaired: false,
                format_repaired: false,
                detail: None,
            });
        }
        ParsedStem::Invalid(detail) => {
            return Ok(StemProbe {
                signal: StemSignal::Invalid,
                repaired: false,
                format_repaired: false,
                detail: Some(detail),
            });
        }
        ParsedStem::Wav(layout) => layout,
    };

    if layout.large_container {
        return Err(format!("{} uses RF64/BW64; preserve the original and run minutes recover-stem with a new output directory before processing", path.display()));
    }

    if layout.actual_data_size == 0 {
        return Ok(StemProbe {
            signal: StemSignal::Empty,
            repaired: false,
            format_repaired: false,
            detail: None,
        });
    }

    let declared_encoding = layout
        .format
        .encoding()
        .ok_or_else(|| "validated WAV format lost its sample encoding".to_string())?;
    let encoding = infer_payload_encoding(path, &layout, declared_encoding)?;
    let format_repaired = encoding != declared_encoding;
    let expected_riff_size = layout
        .file_len
        .checked_sub(8)
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| {
            format!(
                "{} is too large for a standard RIFF/WAVE header; run minutes recover-stem with a new output directory to recover bounded WAV chunks",
                path.display()
            )
        })?;
    let expected_data_size = u32::try_from(layout.actual_data_size).map_err(|_| {
        format!(
            "{} has a data payload too large for a standard RIFF/WAVE header; run minutes recover-stem with a new output directory to recover bounded WAV chunks",
            path.display()
        )
    })?;
    let sizes_inconsistent = layout.declared_riff_size != expected_riff_size
        || layout.declared_data_size != expected_data_size;
    let repaired = sizes_inconsistent || format_repaired;

    if repaired {
        repair_header(
            path,
            &layout,
            expected_riff_size,
            expected_data_size,
            encoding,
            format_repaired,
        )?;
    }

    let signal = if payload_has_audio(path, &layout, encoding)? {
        StemSignal::Usable
    } else {
        StemSignal::Silent
    };

    Ok(StemProbe {
        signal,
        repaired,
        format_repaired,
        detail: None,
    })
}

/// One lossless WAV part and its exact position in the original stem.
#[derive(Debug, serde::Serialize)]
pub struct RecoveredStemPart {
    pub path: std::path::PathBuf,
    pub start_frame: u64,
    pub frames: u64,
    pub pcm_sha256: String,
}

/// A complete recovery receipt, also saved as recovery.json beside the parts.
#[derive(Debug, serde::Serialize)]
pub struct RecoveredStem {
    pub source: std::path::PathBuf,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u64,
    pub parts: Vec<RecoveredStemPart>,
}

/// Copy all PCM into independently processable parts, without changing the input.
/// Refuses active/changed input, an existing destination, partial sample frames,
/// excessive chunk counts, low disk space and work beyond the decode deadline.
/// On failure, complete parts already published remain available; incomplete
/// temporary parts are removed and no completion receipt is published.
pub fn recover_stem(
    path: &Path,
    output_dir: &Path,
    chunk_secs: u64,
) -> Result<RecoveredStem, String> {
    if chunk_secs == 0 || chunk_secs > crate::audio_budget::MAX_AUDIO_SECONDS {
        return Err("chunk seconds must be between 1 and 14400".into());
    }
    let mut input = File::open(path).map_err(|error| error.to_string())?;
    input
        .try_lock_exclusive()
        .map_err(|error| format!("stem is locked or active: {error}"))?;
    let original = input.metadata().map_err(|error| error.to_string())?;
    if !original.is_file() {
        return Err("stem must be a regular file".into());
    }
    let original_modified = original.modified().map_err(|error| error.to_string())?;
    let layout = match parse_wav_file(&mut input, path)? {
        ParsedStem::Wav(layout) => layout,
        ParsedStem::Invalid(detail) => return Err(detail),
        ParsedStem::Missing => return Err("stem is missing".into()),
    };
    crate::audio_budget::AudioWorkBudget::new()
        .validate_stream(
            layout.format.sample_rate,
            usize::from(layout.format.channels),
        )
        .map_err(|error| error.to_string())?;
    let frame_bytes = u64::from(layout.format.block_align);
    if layout.actual_data_size == 0 || !layout.actual_data_size.is_multiple_of(frame_bytes) {
        return Err("stem is empty or ends inside a sample frame; original preserved".into());
    }
    let frames = layout.actual_data_size / frame_bytes;
    let chunk_frames = chunk_secs
        .checked_mul(u64::from(layout.format.sample_rate))
        .ok_or_else(|| "chunk frame count overflow".to_string())?;
    let chunk_bytes = chunk_frames
        .checked_mul(frame_bytes)
        .ok_or_else(|| "chunk byte count overflow".to_string())?;
    // Odd-byte PCM also needs a RIFF padding byte. Include it in the limit.
    if chunk_bytes + 36 + chunk_bytes % 2 > u64::from(u32::MAX) {
        return Err("chunk exceeds RIFF capacity; choose a shorter chunk duration".into());
    }
    let part_count = frames.div_ceil(chunk_frames);
    if part_count > 1_024 {
        return Err("recovery exceeds the 1024-part limit".into());
    }
    let parent = output_dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let required = layout
        .actual_data_size
        .checked_add(part_count * 44 + 1_024 * 1_024 * 1_024)
        .ok_or_else(|| "recovery disk budget overflow".to_string())?;
    if fs2::available_space(parent).map_err(|error| error.to_string())? < required {
        return Err(
            "not enough free disk space for recovery plus a 1 GiB reserve; original preserved"
                .into(),
        );
    }
    // Atomic reservation. No existing folder or part can be overwritten.
    std::fs::create_dir(output_dir)
        .map_err(|error| format!("recovery requires a new output directory: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(output_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    let deadline = std::time::Instant::now() + crate::audio_budget::AUDIO_DECODE_DEADLINE;
    let stable = |input: &File| -> Result<(), String> {
        let current = input.metadata().map_err(|error| error.to_string())?;
        if current.len() != original.len()
            || current.modified().map_err(|error| error.to_string())? != original_modified
        {
            return Err("stem changed during recovery; original preserved, complete parts retained without a completion receipt".into());
        }
        if std::time::Instant::now() >= deadline {
            return Err("stem recovery exceeded its time budget".into());
        }
        Ok(())
    };
    input
        .seek(SeekFrom::Start(layout.data_offset))
        .map_err(|error| error.to_string())?;
    use sha2::{Digest, Sha256};
    let mut buffer = [0_u8; 64 * 1_024];
    let mut receipt = RecoveredStem {
        source: path.to_path_buf(),
        sample_rate: layout.format.sample_rate,
        channels: layout.format.channels,
        frames,
        parts: Vec::new(),
    };
    let encoding = layout
        .format
        .encoding()
        .ok_or_else(|| "unsupported PCM encoding".to_string())?;
    let fields = encoding
        .format_fields(layout.format.channels, layout.format.sample_rate)
        .ok_or_else(|| "PCM format overflow".to_string())?;
    for index in 0..part_count {
        stable(&input)?;
        if fs2::available_space(output_dir).map_err(|error| error.to_string())?
            < 1_024 * 1_024 * 1_024
        {
            return Err(
                "recovery stopped at the 1 GiB disk reserve; complete parts retained".into(),
            );
        }
        let start_frame = index * chunk_frames;
        let part_frames = (frames - start_frame).min(chunk_frames);
        let data_len =
            u32::try_from(part_frames * frame_bytes).map_err(|error| error.to_string())?;
        let mut output =
            tempfile::NamedTempFile::new_in(output_dir).map_err(|error| error.to_string())?;
        let mut header = Vec::with_capacity(44);
        header.extend_from_slice(b"RIFF");
        let riff_size = u32::try_from(36_u64 + u64::from(data_len) + u64::from(data_len % 2))
            .map_err(|_| "recovered part exceeds RIFF capacity".to_string())?;
        header.extend_from_slice(&riff_size.to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16_u32.to_le_bytes());
        header.extend_from_slice(&fields.audio_format.to_le_bytes());
        header.extend_from_slice(&layout.format.channels.to_le_bytes());
        header.extend_from_slice(&layout.format.sample_rate.to_le_bytes());
        header.extend_from_slice(&fields.byte_rate.to_le_bytes());
        header.extend_from_slice(&fields.block_align.to_le_bytes());
        header.extend_from_slice(&fields.bits_per_sample.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&data_len.to_le_bytes());
        output
            .write_all(&header)
            .map_err(|error| error.to_string())?;
        let mut pcm_hash = Sha256::new();
        let mut remaining = u64::from(data_len);
        while remaining > 0 {
            stable(&input)?;
            let bytes = remaining.min(buffer.len() as u64) as usize;
            input
                .read_exact(&mut buffer[..bytes])
                .map_err(|error| error.to_string())?;
            pcm_hash.update(&buffer[..bytes]);
            output
                .write_all(&buffer[..bytes])
                .map_err(|error| error.to_string())?;
            remaining -= bytes as u64;
        }
        if data_len % 2 != 0 {
            output.write_all(&[0]).map_err(|error| error.to_string())?;
        }
        stable(&input)?;
        output
            .as_file()
            .sync_all()
            .map_err(|error| error.to_string())?;
        output
            .as_file_mut()
            .seek(SeekFrom::Start(0))
            .map_err(|error| error.to_string())?;
        {
            let reader =
                hound::WavReader::new(output.as_file_mut()).map_err(|error| error.to_string())?;
            if u64::from(reader.duration()) != part_frames {
                return Err("recovered WAV frame count did not validate".into());
            }
        }
        let part_path = output_dir.join(format!("part-{:04}.wav", index + 1));
        output
            .persist_noclobber(&part_path)
            .map_err(|error| error.to_string())?;
        receipt.parts.push(RecoveredStemPart {
            path: part_path,
            start_frame,
            frames: part_frames,
            pcm_sha256: format!("{:x}", pcm_hash.finalize()),
        });
    }
    stable(&input)?;
    let mut manifest =
        tempfile::NamedTempFile::new_in(output_dir).map_err(|error| error.to_string())?;
    serde_json::to_writer_pretty(&mut manifest, &receipt).map_err(|error| error.to_string())?;
    manifest
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    manifest
        .persist_noclobber(output_dir.join("recovery.json"))
        .map_err(|error| error.to_string())?;
    Ok(receipt)
}

fn parse_wav(path: &Path) -> Result<ParsedStem, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ParsedStem::Missing);
        }
        Err(error) => {
            return Err(format!("could not open {}: {error}", path.display()));
        }
    };
    parse_wav_file(&mut file, path)
}

fn parse_wav_file(file: &mut File, path: &Path) -> Result<ParsedStem, String> {
    let file_len = file
        .metadata()
        .map_err(|error| format!("could not stat {}: {error}", path.display()))?
        .len();
    if file_len < RIFF_HEADER_LEN {
        return Ok(ParsedStem::Invalid(
            "file is shorter than a RIFF header".into(),
        ));
    }

    let mut header = [0_u8; RIFF_HEADER_LEN as usize];
    file.read_exact(&mut header)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let large_container = &header[0..4] == b"RF64" || &header[0..4] == b"BW64";
    if (!large_container && &header[0..4] != b"RIFF") || &header[8..12] != b"WAVE" {
        return Ok(ParsedStem::Invalid(
            "missing RIFF/WAVE signature".to_string(),
        ));
    }
    let declared_riff_size = u32::from_le_bytes(header[4..8].try_into().unwrap());

    let mut ds64: Option<(u64, u64)> = None;
    let mut size_table = Vec::<([u8; 4], u64)>::new();
    let mut chunks = 0_u32;
    let mut offset = RIFF_HEADER_LEN;
    let mut found_format: Option<(u64, FormatContainer, WavFormat)> = None;
    while offset
        .checked_add(CHUNK_HEADER_LEN)
        .is_some_and(|end| end <= file_len)
    {
        chunks += 1;
        if chunks > 1_024 || offset > 16 * 1_024 * 1_024 {
            return Ok(ParsedStem::Invalid(
                "WAV header exceeds the bounded chunk scan".into(),
            ));
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| format!("could not seek {}: {error}", path.display()))?;
        let mut chunk_header = [0_u8; CHUNK_HEADER_LEN as usize];
        file.read_exact(&mut chunk_header)
            .map_err(|error| format!("could not read chunk in {}: {error}", path.display()))?;
        let chunk_id = &chunk_header[0..4];
        let chunk_size = u32::from_le_bytes(chunk_header[4..8].try_into().unwrap());
        let chunk_data_offset = offset + CHUNK_HEADER_LEN;
        if large_container && offset == RIFF_HEADER_LEN && chunk_id != b"ds64" {
            return Ok(ParsedStem::Invalid(
                "RF64/BW64 requires ds64 as its first chunk".into(),
            ));
        }
        if large_container && chunk_id == b"ds64" {
            if ds64.is_some()
                || chunk_size < 28
                || chunk_data_offset + u64::from(chunk_size) > file_len
            {
                return Ok(ParsedStem::Invalid(
                    "invalid or duplicate ds64 chunk".into(),
                ));
            }
            let mut body = [0_u8; 28];
            file.read_exact(&mut body)
                .map_err(|error| error.to_string())?;
            let riff_size = u64::from_le_bytes(body[0..8].try_into().unwrap());
            let data_size = u64::from_le_bytes(body[8..16].try_into().unwrap());
            let table_len = u32::from_le_bytes(body[24..28].try_into().unwrap());
            if table_len > 1_024
                || 28 + u64::from(table_len) * 12 > u64::from(chunk_size)
                || riff_size.checked_add(8) != Some(file_len)
                || declared_riff_size != u32::MAX
            {
                return Ok(ParsedStem::Invalid(
                    "ds64 sizes do not match the finalized file".into(),
                ));
            }
            for _ in 0..table_len {
                let mut entry = [0_u8; 12];
                file.read_exact(&mut entry)
                    .map_err(|error| error.to_string())?;
                size_table.push((
                    entry[0..4].try_into().unwrap(),
                    u64::from_le_bytes(entry[4..12].try_into().unwrap()),
                ));
            }
            ds64 = Some((riff_size, data_size));
        }
        let effective_chunk_size = if chunk_size == u32::MAX && large_container {
            if chunk_id == b"data" {
                ds64.map(|(_, data)| data)
            } else {
                size_table
                    .iter()
                    .position(|(id, _)| id == chunk_id)
                    .map(|index| size_table.remove(index).1)
            }
            .ok_or_else(|| "missing ds64 size for a sentinel chunk".to_string())?
        } else {
            u64::from(chunk_size)
        };

        if chunk_id == b"fmt " && found_format.is_none() {
            if chunk_size < 16 || chunk_data_offset + 16 > file_len {
                return Ok(ParsedStem::Invalid(
                    "fmt chunk is missing its required 16-byte body".into(),
                ));
            }
            let mut fmt = [0_u8; 16];
            file.read_exact(&mut fmt).map_err(|error| {
                format!("could not read fmt chunk in {}: {error}", path.display())
            })?;
            let raw_format_tag = u16::from_le_bytes(fmt[0..2].try_into().unwrap());
            let mut audio_format = raw_format_tag;
            let mut bits_per_sample = u16::from_le_bytes(fmt[14..16].try_into().unwrap());
            let format_container = if raw_format_tag == EXTENSIBLE_FORMAT {
                if chunk_size < 40 || chunk_data_offset + 40 > file_len {
                    return Ok(ParsedStem::Invalid(
                        "extensible fmt chunk is shorter than 40 bytes".into(),
                    ));
                }
                let mut extension = [0_u8; 24];
                file.read_exact(&mut extension).map_err(|error| {
                    format!(
                        "could not read extensible fmt chunk in {}: {error}",
                        path.display()
                    )
                })?;
                if u16::from_le_bytes(extension[0..2].try_into().unwrap()) != 22 {
                    return Ok(ParsedStem::Invalid(
                        "extensible fmt chunk has an invalid extension size".into(),
                    ));
                }
                let valid_bits = u16::from_le_bytes(extension[2..4].try_into().unwrap());
                if valid_bits > 0 {
                    bits_per_sample = valid_bits;
                }
                let subformat: [u8; 16] = extension[8..24].try_into().unwrap();
                audio_format = match subformat {
                    PCM_SUBFORMAT => PCM_FORMAT,
                    FLOAT_SUBFORMAT => IEEE_FLOAT_FORMAT,
                    _ => {
                        return Ok(ParsedStem::Invalid(
                            "extensible fmt chunk has an unsupported PCM subformat".into(),
                        ));
                    }
                };
                FormatContainer::Extensible {
                    subformat_offset: chunk_data_offset + 24,
                }
            } else {
                FormatContainer::Classic
            };
            let format = WavFormat {
                audio_format,
                channels: u16::from_le_bytes(fmt[2..4].try_into().unwrap()),
                sample_rate: u32::from_le_bytes(fmt[4..8].try_into().unwrap()),
                byte_rate: u32::from_le_bytes(fmt[8..12].try_into().unwrap()),
                block_align: u16::from_le_bytes(fmt[12..14].try_into().unwrap()),
                bits_per_sample,
            };
            if !format.is_sane() {
                return Ok(ParsedStem::Invalid(
                    "fmt chunk has unsupported or internally inconsistent PCM fields".into(),
                ));
            }
            found_format = Some((chunk_data_offset, format_container, format));
        }

        if chunk_id == b"data" {
            let Some((fmt_offset, format_container, format)) = found_format else {
                return Ok(ParsedStem::Invalid(
                    "data chunk appears before a valid fmt chunk".into(),
                ));
            };
            let actual_data_size = if large_container {
                // Respect the declared data extent. Trailing metadata is not PCM.
                if chunk_data_offset
                    .checked_add(effective_chunk_size)
                    .and_then(|end| end.checked_add(effective_chunk_size % 2))
                    .is_none_or(|end| end > file_len)
                {
                    return Ok(ParsedStem::Invalid("ds64 data extends beyond EOF".into()));
                }
                effective_chunk_size
            } else if u64::from(declared_riff_size) + 8 == file_len
                && chunk_size > 0
                && chunk_data_offset + u64::from(chunk_size) <= file_len
            {
                u64::from(chunk_size)
            } else {
                file_len.saturating_sub(chunk_data_offset)
            };
            return Ok(ParsedStem::Wav(WavLayout {
                file_len,
                declared_riff_size,
                fmt_offset,
                format_container,
                format,
                data_size_offset: offset + 4,
                data_offset: chunk_data_offset,
                declared_data_size: chunk_size,
                actual_data_size,
                large_container,
            }));
        }

        let padded_size = effective_chunk_size
            .checked_add(effective_chunk_size % 2)
            .ok_or_else(|| format!("chunk size overflow in {}", path.display()))?;
        let Some(next_offset) = chunk_data_offset.checked_add(padded_size) else {
            return Ok(ParsedStem::Invalid("chunk offset overflow".into()));
        };
        if next_offset > file_len {
            return Ok(ParsedStem::Invalid(format!(
                "{} chunk extends beyond EOF",
                String::from_utf8_lossy(chunk_id)
            )));
        }
        offset = next_offset;
    }

    Ok(ParsedStem::Invalid(
        "no data chunk follows the fmt chunk".into(),
    ))
}

#[derive(Debug, Default)]
struct FloatEvidence {
    total: u64,
    bounded: u64,
    nonzero: u64,
    sum_sq: f64,
    max_abs: f32,
}

impl FloatEvidence {
    fn observe(&mut self, value: f32) {
        self.total += 1;
        if value.is_finite() && value.abs() <= 8.0 {
            self.bounded += 1;
            let abs = value.abs();
            if abs > 0.000_001 {
                self.nonzero += 1;
            }
            self.sum_sq += f64::from(value) * f64::from(value);
            self.max_abs = self.max_abs.max(abs);
        }
    }

    fn is_strong_float(&self) -> bool {
        self.total >= FORMAT_EVIDENCE_MIN_SAMPLES
            && self.bounded.saturating_mul(1_000) >= self.total.saturating_mul(999)
            && self.nonzero >= FORMAT_EVIDENCE_MIN_NONZERO
            && self.max_abs > STEM_PROBE_RMS_FLOOR
            && self.rms() <= 2.0
    }

    fn is_clearly_not_float(&self) -> bool {
        self.total >= FORMAT_EVIDENCE_MIN_SAMPLES
            && self.bounded.saturating_mul(100) < self.total.saturating_mul(95)
    }

    fn rms(&self) -> f32 {
        if self.bounded == 0 {
            return 0.0;
        }
        (self.sum_sq / self.bounded as f64).sqrt() as f32
    }
}

fn infer_payload_encoding(
    path: &Path,
    layout: &WavLayout,
    declared: SampleEncoding,
) -> Result<SampleEncoding, String> {
    match declared {
        SampleEncoding::Signed16 | SampleEncoding::Signed32 => {
            let float_frame_bytes = 4_u64 * u64::from(layout.format.channels);
            if !layout.actual_data_size.is_multiple_of(float_frame_bytes) {
                return Ok(declared);
            }
            let evidence = scan_float_evidence(path, layout)?;
            if evidence.is_strong_float() {
                Ok(SampleEncoding::Float32)
            } else {
                Ok(declared)
            }
        }
        SampleEncoding::Float32 => {
            let evidence = scan_float_evidence(path, layout)?;
            let int16_frame_bytes = 2_u64 * u64::from(layout.format.channels);
            if evidence.is_clearly_not_float()
                && layout.actual_data_size.is_multiple_of(int16_frame_bytes)
                && payload_has_audio(path, layout, SampleEncoding::Signed16)?
            {
                Ok(SampleEncoding::Signed16)
            } else if evidence.total > 0 && evidence.bounded == 0 {
                Err(format!(
                    "{} payload cannot be decoded as the declared float32 format",
                    path.display()
                ))
            } else {
                Ok(declared)
            }
        }
        SampleEncoding::Unsigned8 | SampleEncoding::Signed24 => Ok(declared),
    }
}

fn scan_float_evidence(path: &Path, layout: &WavLayout) -> Result<FloatEvidence, String> {
    let mut reader = payload_reader(path, layout)?;
    let mut remaining = layout.actual_data_size;
    let mut evidence = FloatEvidence::default();
    let mut buffer = [0_u8; 64 * 1_024];

    while remaining >= 4 {
        let complete_bytes = remaining - (remaining % 4);
        let to_read = usize::try_from(complete_bytes.min(buffer.len() as u64))
            .map_err(|_| format!("payload chunk is too large in {}", path.display()))?;
        reader
            .read_exact(&mut buffer[..to_read])
            .map_err(|error| format!("could not read payload from {}: {error}", path.display()))?;
        remaining -= to_read as u64;
        for bytes in buffer[..to_read].chunks_exact(4) {
            evidence.observe(f32::from_le_bytes(bytes.try_into().unwrap()));
        }
        if evidence.is_strong_float() || evidence.is_clearly_not_float() {
            break;
        }
    }
    Ok(evidence)
}

fn payload_reader(path: &Path, layout: &WavLayout) -> Result<BufReader<File>, String> {
    let mut file =
        File::open(path).map_err(|error| format!("could not open {}: {error}", path.display()))?;
    file.seek(SeekFrom::Start(layout.data_offset))
        .map_err(|error| format!("could not seek to payload in {}: {error}", path.display()))?;
    Ok(BufReader::new(file))
}

fn payload_has_audio(
    path: &Path,
    layout: &WavLayout,
    encoding: SampleEncoding,
) -> Result<bool, String> {
    let bytes_per_sample = encoding.bytes_per_sample();
    let frame_bytes = bytes_per_sample
        .checked_mul(usize::from(layout.format.channels))
        .ok_or_else(|| format!("sample frame size overflow in {}", path.display()))?;
    if frame_bytes == 0 {
        return Ok(false);
    }

    let mut reader = payload_reader(path, layout)?;
    let mut remaining = layout.actual_data_size;
    let mut buffer_len = 64 * 1_024;
    buffer_len -= buffer_len % frame_bytes;
    if buffer_len == 0 {
        buffer_len = frame_bytes;
    }
    let mut buffer = vec![0_u8; buffer_len];
    let window_frames = usize::try_from(layout.format.sample_rate)
        .map_err(|_| format!("sample rate is too large in {}", path.display()))?;
    let mut window_frames_read = 0_usize;
    let mut window_sum_sq = 0.0_f64;

    while remaining >= frame_bytes as u64 {
        let complete_bytes = remaining - (remaining % frame_bytes as u64);
        let to_read = usize::try_from(complete_bytes.min(buffer.len() as u64))
            .map_err(|_| format!("payload chunk is too large in {}", path.display()))?;
        reader
            .read_exact(&mut buffer[..to_read])
            .map_err(|error| format!("could not read payload from {}: {error}", path.display()))?;
        remaining -= to_read as u64;

        for frame in buffer[..to_read].chunks_exact(frame_bytes) {
            let mut mono = 0.0_f32;
            for sample in frame.chunks_exact(bytes_per_sample) {
                mono += decode_sample(sample, encoding);
            }
            mono /= f32::from(layout.format.channels);
            if !mono.is_finite() {
                continue;
            }
            window_sum_sq += f64::from(mono) * f64::from(mono);
            window_frames_read += 1;

            if window_frames_read >= window_frames {
                let rms = (window_sum_sq / window_frames_read as f64).sqrt() as f32;
                if rms > STEM_PROBE_RMS_FLOOR {
                    return Ok(true);
                }
                window_frames_read = 0;
                window_sum_sq = 0.0;
            }
        }
    }

    if window_frames_read == 0 {
        return Ok(false);
    }
    let rms = (window_sum_sq / window_frames_read as f64).sqrt() as f32;
    Ok(rms > STEM_PROBE_RMS_FLOOR)
}

fn decode_sample(bytes: &[u8], encoding: SampleEncoding) -> f32 {
    match encoding {
        SampleEncoding::Unsigned8 => (f32::from(bytes[0]) - 128.0) / 128.0,
        SampleEncoding::Signed16 => {
            f32::from(i16::from_le_bytes(bytes.try_into().unwrap())) / 32_768.0
        }
        SampleEncoding::Signed24 => {
            let sign = if bytes[2] & 0x80 == 0 { 0 } else { 0xff };
            let raw = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], sign]);
            raw as f32 / 8_388_608.0
        }
        SampleEncoding::Signed32 => {
            i32::from_le_bytes(bytes.try_into().unwrap()) as f32 / 2_147_483_648.0
        }
        SampleEncoding::Float32 => f32::from_le_bytes(bytes.try_into().unwrap()),
    }
}

fn repair_header(
    path: &Path,
    layout: &WavLayout,
    riff_size: u32,
    data_size: u32,
    encoding: SampleEncoding,
    format_repaired: bool,
) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| {
            format!(
                "could not open {} for header repair: {error}",
                path.display()
            )
        })?;
    file.try_lock_exclusive().map_err(|error| {
        format!(
            "could not lock {} for header repair: {error}",
            path.display()
        )
    })?;
    let locked_len = file
        .metadata()
        .map_err(|error| format!("could not stat locked stem {}: {error}", path.display()))?
        .len();
    if locked_len != layout.file_len {
        return Err(format!(
            "{} changed while it was being probed; refusing to repair an active or unstable stem",
            path.display()
        ));
    }

    let format_fields = encoding
        .format_fields(layout.format.channels, layout.format.sample_rate)
        .ok_or_else(|| format!("repaired fmt fields overflow for {}", path.display()))?;
    let mut patches = vec![
        (4_u64, riff_size.to_le_bytes().to_vec()),
        (layout.data_size_offset, data_size.to_le_bytes().to_vec()),
    ];
    if format_repaired {
        match layout.format_container {
            FormatContainer::Classic => {
                patches.push((
                    layout.fmt_offset,
                    format_fields.audio_format.to_le_bytes().to_vec(),
                ));
            }
            FormatContainer::Extensible { subformat_offset } => {
                let subformat = match encoding {
                    SampleEncoding::Float32 => FLOAT_SUBFORMAT,
                    SampleEncoding::Unsigned8
                    | SampleEncoding::Signed16
                    | SampleEncoding::Signed24
                    | SampleEncoding::Signed32 => PCM_SUBFORMAT,
                };
                patches.push((subformat_offset, subformat.to_vec()));
                patches.push((
                    layout.fmt_offset + 18,
                    format_fields.bits_per_sample.to_le_bytes().to_vec(),
                ));
            }
        }
        patches.extend([
            (
                layout.fmt_offset + 8,
                format_fields.byte_rate.to_le_bytes().to_vec(),
            ),
            (
                layout.fmt_offset + 12,
                format_fields.block_align.to_le_bytes().to_vec(),
            ),
            (
                layout.fmt_offset + 14,
                format_fields.bits_per_sample.to_le_bytes().to_vec(),
            ),
        ]);
    }

    let mut originals = Vec::with_capacity(patches.len());
    for (offset, replacement) in &patches {
        file.seek(SeekFrom::Start(*offset)).map_err(|error| {
            format!(
                "could not seek in {} for header repair: {error}",
                path.display()
            )
        })?;
        let mut original = vec![0_u8; replacement.len()];
        file.read_exact(&mut original).map_err(|error| {
            format!(
                "could not verify original header bytes in {}: {error}",
                path.display()
            )
        })?;
        originals.push((*offset, original));
    }
    if originals[0].1 != layout.declared_riff_size.to_le_bytes()
        || originals[1].1 != layout.declared_data_size.to_le_bytes()
    {
        return Err(format!(
            "{} header changed while it was being probed; refusing to overwrite newer header fields",
            path.display()
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| {
                format!(
                    "could not preserve 0600 permissions on {} during header repair: {error}",
                    path.display()
                )
            })?;
    }

    let repair_result = (|| -> Result<(), String> {
        for (offset, replacement) in &patches {
            file.seek(SeekFrom::Start(*offset))
                .map_err(|error| error.to_string())?;
            file.write_all(replacement)
                .map_err(|error| error.to_string())?;
        }
        file.sync_all().map_err(|error| error.to_string())?;
        // Validate through the SAME locked handle: opening a second handle
        // here deadlocks against our own exclusive lock on Windows, where
        // file locks are mandatory rather than advisory (os error 33).
        file.seek(SeekFrom::Start(0))
            .map_err(|error| error.to_string())?;
        hound::WavReader::new(std::io::BufReader::new(&mut file))
            .map(drop)
            .map_err(|error| format!("repaired WAV did not pass decoder validation: {error}"))
    })();

    if let Err(repair_error) = repair_result {
        let rollback_result = (|| -> std::io::Result<()> {
            for (offset, original) in &originals {
                file.seek(SeekFrom::Start(*offset))?;
                file.write_all(original)?;
            }
            file.sync_all()
        })();
        return Err(match rollback_result {
            Ok(()) => format!(
                "could not repair {} header; original header was restored: {repair_error}",
                path.display()
            ),
            Err(rollback_error) => format!(
                "could not repair {} header ({repair_error}) and could not restore its original header ({rollback_error}); audio payload bytes were not touched",
                path.display()
            ),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const DATA_OFFSET: usize = 0x1000;

    fn tone_samples() -> Vec<f32> {
        (0..48_000)
            .map(|sample| (2.0 * PI * 440.0 * sample as f32 / 48_000.0).sin() * 0.25)
            .collect()
    }

    fn write_placeholder_f32_wav(
        path: &Path,
        samples: &[f32],
        declared_encoding: SampleEncoding,
        declared_data_size: u32,
    ) {
        let mut bytes = vec![0_u8; DATA_OFFSET];
        bytes[0..4].copy_from_slice(b"RIFF");
        bytes[4..8].copy_from_slice(&4_088_u32.to_le_bytes());
        bytes[8..12].copy_from_slice(b"WAVE");
        bytes[12..16].copy_from_slice(b"JUNK");
        bytes[16..20].copy_from_slice(&28_u32.to_le_bytes());
        bytes[48..52].copy_from_slice(b"fmt ");
        bytes[52..56].copy_from_slice(&16_u32.to_le_bytes());
        let format = declared_encoding
            .format_fields(1, 48_000)
            .expect("test format must be representable");
        bytes[56..58].copy_from_slice(&format.audio_format.to_le_bytes());
        bytes[58..60].copy_from_slice(&1_u16.to_le_bytes());
        bytes[60..64].copy_from_slice(&48_000_u32.to_le_bytes());
        bytes[64..68].copy_from_slice(&format.byte_rate.to_le_bytes());
        bytes[68..70].copy_from_slice(&format.block_align.to_le_bytes());
        bytes[70..72].copy_from_slice(&format.bits_per_sample.to_le_bytes());
        bytes[72..76].copy_from_slice(b"FLLR");
        bytes[76..80].copy_from_slice(&4_008_u32.to_le_bytes());
        bytes[4_088..4_092].copy_from_slice(b"data");
        bytes[4_092..4_096].copy_from_slice(&declared_data_size.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    fn read_u16(bytes: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
    }

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    fn large_wave(kind: &[u8; 4], samples: &[f32], trailing: bool) -> Vec<u8> {
        let data_size = (samples.len() * 4) as u64;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(b"WAVEds64");
        bytes.extend_from_slice(&28_u32.to_le_bytes());
        bytes.extend_from_slice(&(72 + data_size + if trailing { 12 } else { 0 }).to_le_bytes());
        bytes.extend_from_slice(&data_size.to_le_bytes());
        bytes.extend_from_slice(&(samples.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&3_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&192_000_u32.to_le_bytes());
        bytes.extend_from_slice(&4_u16.to_le_bytes());
        bytes.extend_from_slice(&32_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        if trailing {
            bytes.extend_from_slice(b"JUNK\x04\x00\x00\x00tail");
        }
        bytes
    }

    #[test]
    fn rf64_and_bw64_recovery_preserve_every_pcm_byte_and_exclude_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let samples: Vec<f32> = (0..120_123)
            .map(|n| ((n % 100) as f32 - 50.0) / 100.0)
            .collect();
        for kind in [b"RF64", b"BW64"] {
            let path = dir
                .path()
                .join(format!("{}.wav", String::from_utf8_lossy(kind)));
            let original = large_wave(kind, &samples, true);
            std::fs::write(&path, &original).unwrap();
            let output = path.with_extension("parts");
            let receipt = recover_stem(&path, &output, 1).unwrap();
            assert_eq!(receipt.frames, samples.len() as u64);
            assert_eq!(receipt.parts.len(), 3);
            let mut recovered = Vec::new();
            for (index, part) in receipt.parts.iter().enumerate() {
                assert_eq!(part.start_frame, index as u64 * 48_000);
                let mut reader = hound::WavReader::open(&part.path).unwrap();
                assert_eq!(reader.spec().sample_format, hound::SampleFormat::Float);
                recovered.extend(reader.samples::<f32>().map(Result::unwrap));
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    assert_eq!(
                        std::fs::metadata(&part.path).unwrap().permissions().mode() & 0o777,
                        0o600
                    );
                }
            }
            assert_eq!(recovered, samples);
            assert_eq!(std::fs::read(&path).unwrap(), original);
            assert!(output.join("recovery.json").exists());
            assert!(recover_stem(&path, &output, 1)
                .unwrap_err()
                .contains("new output directory"));
            assert_eq!(std::fs::read(&path).unwrap(), original);
        }
    }

    #[test]
    fn odd_pcm8_parts_include_padding_without_adding_audio_frames() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("odd.wav");
        let samples = [0_i8, 127, -128, 1, 2, 52, 62];
        let mut writer = hound::WavWriter::create(
            &path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 3,
                bits_per_sample: 8,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for sample in samples {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        let original = std::fs::read(&path).unwrap();
        let receipt = recover_stem(&path, &dir.path().join("parts"), 1).unwrap();
        assert_eq!(receipt.parts.len(), 3);
        let mut decoded = Vec::new();
        for part in &receipt.parts {
            let bytes = std::fs::read(&part.path).unwrap();
            assert_eq!(u64::from(read_u32(&bytes, 4)) + 8, bytes.len() as u64);
            assert_eq!(read_u32(&bytes, 40) as u64, part.frames);
            assert_eq!(bytes.len() as u64, 44 + part.frames + part.frames % 2);
            decoded.extend(
                hound::WavReader::open(&part.path)
                    .unwrap()
                    .samples::<i8>()
                    .map(Result::unwrap),
            );
        }
        assert_eq!(decoded, samples);
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn oversized_sparse_riff_and_rf64_sizes_are_64_bit_and_sources_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let data_size = u64::from(u32::MAX) + 5;
        for large in [false, true] {
            let path = dir.path().join(format!("large-{large}.wav"));
            let mut bytes = large_wave(b"RF64", &[], false);
            if large {
                bytes[20..28].copy_from_slice(&(data_size + 72).to_le_bytes());
                bytes[28..36].copy_from_slice(&data_size.to_le_bytes());
            } else {
                bytes[0..4].copy_from_slice(b"RIFF");
                bytes[12..16].copy_from_slice(b"JUNK");
            }
            let mut file = File::create(&path).unwrap();
            file.write_all(&bytes).unwrap();
            file.set_len(80 + data_size).unwrap();
            drop(file);
            let layout = match parse_wav(&path).unwrap() {
                ParsedStem::Wav(layout) => layout,
                other => panic!("{other:?}"),
            };
            assert_eq!(layout.actual_data_size, data_size);
            assert_eq!(layout.large_container, large);
            let mut header = [0; 80];
            File::open(&path).unwrap().read_exact(&mut header).unwrap();
            assert_eq!(header.as_slice(), bytes);
        }
    }

    #[test]
    fn corrupt_ds64_and_partial_frames_do_not_publish_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.wav");
        let valid = large_wave(b"RF64", &[0.25; 100], false);
        for fault in 0..5 {
            let mut bytes = valid.clone();
            match fault {
                0 => bytes[12..16].copy_from_slice(b"JUNK"),
                1 => bytes[28..36].copy_from_slice(&u64::MAX.to_le_bytes()),
                2 => bytes[44..48].copy_from_slice(&u32::MAX.to_le_bytes()),
                3 => bytes[20..28].copy_from_slice(&0_u64.to_le_bytes()),
                _ => bytes[28..36].copy_from_slice(&399_u64.to_le_bytes()),
            }
            std::fs::write(&path, &bytes).unwrap();
            let output = dir.path().join(format!("out-{fault}"));
            assert!(recover_stem(&path, &output, 1).is_err());
            assert!(!output.exists());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn placeholder_riff_recovery_preserves_payload_and_refuses_locked_sources() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unfinalized.wav");
        let samples = tone_samples();
        write_placeholder_f32_wav(&path, &samples, SampleEncoding::Float32, 0);
        let original = std::fs::read(&path).unwrap();
        let locked = File::open(&path).unwrap();
        locked.try_lock_exclusive().unwrap();
        let out = dir.path().join("parts");
        assert!(recover_stem(&path, &out, 1).is_err());
        assert!(!out.exists());
        drop(locked);
        let receipt = recover_stem(&path, &out, 1).unwrap();
        let recovered = std::fs::read(&receipt.parts[0].path).unwrap();
        assert_eq!(&recovered[44..], &original[DATA_OFFSET..]);
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn accepts_and_repairs_unfinalized_f32_with_apple_reserve_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("placeholder.wav");
        let samples = tone_samples();
        write_placeholder_f32_wav(&path, &samples, SampleEncoding::Float32, 0);
        let before = std::fs::read(&path).unwrap();

        let probe = probe_and_repair(&path).unwrap();

        assert_eq!(probe.signal, StemSignal::Usable);
        assert!(probe.repaired);
        assert!(!probe.format_repaired);
        let after = std::fs::read(&path).unwrap();
        assert_eq!(
            &after[DATA_OFFSET..],
            &before[DATA_OFFSET..],
            "header repair must not rewrite payload bytes"
        );
        assert_eq!(read_u32(&after, 4), (after.len() - 8) as u32);
        assert_eq!(
            read_u32(&after, DATA_OFFSET - 4),
            (after.len() - DATA_OFFSET) as u32
        );
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.duration(), samples.len() as u32);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn inconsistent_nonzero_data_size_is_measured_to_eof_and_repaired() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("short-declared.wav");
        let samples = tone_samples();
        write_placeholder_f32_wav(&path, &samples, SampleEncoding::Float32, 4);

        let probe = probe_and_repair(&path).unwrap();

        assert_eq!(probe.signal, StemSignal::Usable);
        assert!(probe.repaired);
        let after = std::fs::read(&path).unwrap();
        assert_eq!(
            read_u32(&after, DATA_OFFSET - 4),
            (samples.len() * std::mem::size_of::<f32>()) as u32
        );
    }

    #[test]
    fn all_zero_payload_is_repaired_but_still_reported_silent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("silent-placeholder.wav");
        let samples = vec![0.0_f32; 48_000];
        write_placeholder_f32_wav(&path, &samples, SampleEncoding::Float32, 0);

        let probe = probe_and_repair(&path).unwrap();

        assert_eq!(probe.signal, StemSignal::Silent);
        assert!(probe.repaired);
        let after = std::fs::read(&path).unwrap();
        assert_eq!(
            read_u32(&after, DATA_OFFSET - 4),
            (samples.len() * std::mem::size_of::<f32>()) as u32
        );
    }

    #[test]
    fn repairs_int16_header_that_describes_f32_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mismatched-format.wav");
        let samples = tone_samples();
        write_placeholder_f32_wav(&path, &samples, SampleEncoding::Signed16, 0);

        let probe = probe_and_repair(&path).unwrap();

        assert_eq!(probe.signal, StemSignal::Usable);
        assert!(probe.repaired);
        assert!(probe.format_repaired);
        let after = std::fs::read(&path).unwrap();
        assert_eq!(read_u16(&after, 56), IEEE_FLOAT_FORMAT);
        assert_eq!(read_u32(&after, 64), 192_000);
        assert_eq!(read_u16(&after, 68), 4);
        assert_eq!(read_u16(&after, 70), 32);
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().sample_format, hound::SampleFormat::Float);
        assert_eq!(reader.duration(), samples.len() as u32);
    }

    #[test]
    fn malformed_header_is_rejected_without_modification() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-wave.wav");
        std::fs::write(&path, b"not a wave file").unwrap();
        let before = std::fs::read(&path).unwrap();

        let probe = probe_and_repair(&path).unwrap();

        assert_eq!(probe.signal, StemSignal::Invalid);
        assert!(!probe.repaired);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn valid_int16_stem_is_not_mistaken_for_float_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("valid-int16.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for sample in 0..16_000 {
            let value = (5_000.0 * (2.0 * PI * 440.0 * sample as f32 / 16_000.0).sin()) as i16;
            writer.write_sample(value).unwrap();
        }
        writer.finalize().unwrap();
        let before = std::fs::read(&path).unwrap();

        let probe = probe_and_repair(&path).unwrap();

        assert_eq!(probe.signal, StemSignal::Usable);
        assert!(!probe.repaired);
        assert!(!probe.format_repaired);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn repairs_extensible_float_sizes_without_rewriting_subformat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("extensible-float.wav");
        let samples = tone_samples();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for sample in &samples {
            writer.write_sample(*sample).unwrap();
        }
        writer.finalize().unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        assert_eq!(read_u16(&bytes, 20), EXTENSIBLE_FORMAT);
        let subformat_before = bytes[44..60].to_vec();
        bytes[4..8].copy_from_slice(&4_088_u32.to_le_bytes());
        bytes[64..68].copy_from_slice(&0_u32.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();

        let probe = probe_and_repair(&path).unwrap();

        assert_eq!(probe.signal, StemSignal::Usable);
        assert!(probe.repaired);
        assert!(!probe.format_repaired);
        let after = std::fs::read(&path).unwrap();
        assert_eq!(read_u16(&after, 20), EXTENSIBLE_FORMAT);
        assert_eq!(&after[44..60], subformat_before);
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().sample_format, hound::SampleFormat::Float);
        assert_eq!(reader.duration(), samples.len() as u32);
    }
}
