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
                "{} is too large for a standard RIFF/WAVE header; RF64 recovery is required",
                path.display()
            )
        })?;
    let expected_data_size = u32::try_from(layout.actual_data_size).map_err(|_| {
        format!(
            "{} has a data payload too large for a standard RIFF/WAVE header; RF64 recovery is required",
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
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Ok(ParsedStem::Invalid(
            "missing RIFF/WAVE signature".to_string(),
        ));
    }
    let declared_riff_size = u32::from_le_bytes(header[4..8].try_into().unwrap());

    let mut offset = RIFF_HEADER_LEN;
    let mut found_format: Option<(u64, FormatContainer, WavFormat)> = None;
    while offset
        .checked_add(CHUNK_HEADER_LEN)
        .is_some_and(|end| end <= file_len)
    {
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| format!("could not seek {}: {error}", path.display()))?;
        let mut chunk_header = [0_u8; CHUNK_HEADER_LEN as usize];
        file.read_exact(&mut chunk_header)
            .map_err(|error| format!("could not read chunk in {}: {error}", path.display()))?;
        let chunk_id = &chunk_header[0..4];
        let chunk_size = u32::from_le_bytes(chunk_header[4..8].try_into().unwrap());
        let chunk_data_offset = offset + CHUNK_HEADER_LEN;

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
            let actual_data_size = file_len.saturating_sub(chunk_data_offset);
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
            }));
        }

        let padded_size = u64::from(chunk_size)
            .checked_add(u64::from(chunk_size % 2))
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
