//! Authenticated, constant-working-memory storage for private audio.
//!
//! Darwin has no `O_TMPFILE`, and its POSIX-shm descriptors are mmap-only:
//! ordinary `read`, `write`, and `lseek` fail. A create-then-unlink regular
//! file is seekable, but a hostile same-UID process can open the short-lived
//! name and retain that descriptor. This store makes that race harmless: the
//! temporary file contains only independently authenticated ciphertext, is
//! unlinked before the first byte is written, and the random key never leaves
//! this process. Windows uses a retained, non-inheritable delete-on-close
//! handle with the same encrypted representation. Readers decrypt one fixed-
//! size chunk at a time.

#![cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]

use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce, Tag};
use std::fs::File;
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use zeroize::{Zeroize, Zeroizing};

const PLAINTEXT_CHUNK_BYTES: usize = 32 * 1024;
const TAG_BYTES: usize = 16;
const CIPHERTEXT_CHUNK_BYTES: u64 = (PLAINTEXT_CHUNK_BYTES + TAG_BYTES) as u64;
const AAD_DOMAIN: &[u8] = b"minutes-private-audio-v1";
const MAX_PLAINTEXT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Production consumers normally hold one reader per capability; independent
/// cursor users can legitimately hold two. Leave fixed headroom for bounded
/// probes and decoders without allowing a caller to multiply the per-reader
/// 32 KiB plaintext cache without limit. Voice/system recovery uses one reader
/// from each of two distinct capabilities rather than sharing this allowance.
const MAX_ACTIVE_READERS: usize = 8;

struct State {
    file: File,
    key_bytes: Zeroizing<[u8; 32]>,
    nonce_prefix: [u8; 4],
    object_id: [u8; 16],
    generation: u64,
    pending: Zeroizing<Vec<u8>>,
    plaintext_len: u64,
    sealed: bool,
    poisoned: bool,
    writer_issued: bool,
    writer_active: bool,
    active_readers: usize,
}

impl State {
    fn new(file: File) -> std::io::Result<Self> {
        let mut key_bytes = Zeroizing::new([0u8; 32]);
        let mut nonce_prefix = [0u8; 4];
        let mut object_id = [0u8; 16];
        getrandom::fill(&mut *key_bytes)
            .map_err(|error| std::io::Error::other(format!("audio key failed: {error}")))?;
        getrandom::fill(&mut nonce_prefix)
            .map_err(|error| std::io::Error::other(format!("audio nonce failed: {error}")))?;
        getrandom::fill(&mut object_id)
            .map_err(|error| std::io::Error::other(format!("audio object id failed: {error}")))?;
        Ok(Self {
            file,
            key_bytes,
            nonce_prefix,
            object_id,
            generation: 1,
            pending: Zeroizing::new(Vec::with_capacity(PLAINTEXT_CHUNK_BYTES)),
            plaintext_len: 0,
            sealed: false,
            poisoned: false,
            writer_issued: false,
            writer_active: false,
            active_readers: 0,
        })
    }

    fn cipher(&self) -> ChaCha20Poly1305 {
        ChaCha20Poly1305::new(Key::from_slice(self.key_bytes.as_ref()))
    }

    fn nonce(&self, chunk_index: u64) -> Nonce {
        let mut bytes = [0u8; 12];
        bytes[..4].copy_from_slice(&self.nonce_prefix);
        bytes[4..].copy_from_slice(&chunk_index.to_be_bytes());
        Nonce::clone_from_slice(&bytes)
    }

    fn aad(&self, chunk_index: u64) -> Vec<u8> {
        let mut aad = Vec::with_capacity(AAD_DOMAIN.len() + 16 + 8 + 8);
        aad.extend_from_slice(AAD_DOMAIN);
        aad.extend_from_slice(&self.object_id);
        aad.extend_from_slice(&self.generation.to_be_bytes());
        aad.extend_from_slice(&chunk_index.to_be_bytes());
        aad
    }

    fn expected_ciphertext_len(&self) -> std::io::Result<u64> {
        let chunks = self
            .plaintext_len
            .checked_add(PLAINTEXT_CHUNK_BYTES as u64 - 1)
            .ok_or_else(|| std::io::Error::other("private audio chunk count overflowed"))?
            / PLAINTEXT_CHUNK_BYTES as u64;
        chunks
            .checked_mul(CIPHERTEXT_CHUNK_BYTES)
            .ok_or_else(|| std::io::Error::other("private audio ciphertext length overflowed"))
    }

    fn attest_ciphertext_len(&self) -> std::io::Result<()> {
        if self.file.metadata()?.len() != self.expected_ciphertext_len()? {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "private audio ciphertext length changed",
            ));
        }
        Ok(())
    }

    fn write_ciphertext_at(&self, mut bytes: &[u8], mut offset: u64) -> std::io::Result<()> {
        while !bytes.is_empty() {
            #[cfg(unix)]
            let written = {
                use std::os::unix::fs::FileExt;
                self.file.write_at(bytes, offset)?
            };
            #[cfg(windows)]
            let written = {
                use std::os::windows::fs::FileExt;
                self.file.seek_write(bytes, offset)?
            };
            if written == 0 {
                return Err(std::io::Error::new(
                    ErrorKind::WriteZero,
                    "private audio ciphertext write stopped",
                ));
            }
            bytes = &bytes[written..];
            offset = offset.saturating_add(written as u64);
        }
        Ok(())
    }

    fn read_ciphertext_at(&self, mut bytes: &mut [u8], mut offset: u64) -> std::io::Result<()> {
        while !bytes.is_empty() {
            #[cfg(unix)]
            let read = {
                use std::os::unix::fs::FileExt;
                self.file.read_at(bytes, offset)?
            };
            #[cfg(windows)]
            let read = {
                use std::os::windows::fs::FileExt;
                self.file.seek_read(bytes, offset)?
            };
            if read == 0 {
                return Err(std::io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "private audio ciphertext was truncated",
                ));
            }
            let (_, rest) = bytes.split_at_mut(read);
            bytes = rest;
            offset = offset.saturating_add(read as u64);
        }
        Ok(())
    }

    fn seal_pending_chunk(&mut self) -> std::io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let chunk_index =
            (self.plaintext_len - self.pending.len() as u64) / PLAINTEXT_CHUNK_BYTES as u64;
        let mut ciphertext = Zeroizing::new(vec![0u8; PLAINTEXT_CHUNK_BYTES + TAG_BYTES]);
        ciphertext[..self.pending.len()].copy_from_slice(&self.pending);
        let tag = self
            .cipher()
            .encrypt_in_place_detached(
                &self.nonce(chunk_index),
                &self.aad(chunk_index),
                &mut ciphertext[..PLAINTEXT_CHUNK_BYTES],
            )
            .map_err(|_| {
                self.poisoned = true;
                std::io::Error::other("private audio encryption failed")
            })?;
        ciphertext[PLAINTEXT_CHUNK_BYTES..].copy_from_slice(&tag);
        if let Err(error) =
            self.write_ciphertext_at(&ciphertext, chunk_index * CIPHERTEXT_CHUNK_BYTES)
        {
            self.poisoned = true;
            return Err(error);
        }
        self.pending.zeroize();
        self.pending.clear();
        Ok(())
    }

    fn append(&mut self, mut bytes: &[u8]) -> std::io::Result<usize> {
        if self.sealed || self.poisoned || !self.writer_active {
            return Err(std::io::Error::other("private audio writer is not active"));
        }
        let next_len = self
            .plaintext_len
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| std::io::Error::other("private audio length overflowed"))?;
        if next_len > MAX_PLAINTEXT_BYTES {
            self.poisoned = true;
            return Err(std::io::Error::other(
                "private audio output resource budget exceeded",
            ));
        }
        let original_len = bytes.len();
        while !bytes.is_empty() {
            let available = PLAINTEXT_CHUNK_BYTES - self.pending.len();
            let take = available.min(bytes.len());
            self.pending.extend_from_slice(&bytes[..take]);
            self.plaintext_len = self
                .plaintext_len
                .checked_add(take as u64)
                .ok_or_else(|| std::io::Error::other("private audio length overflowed"))?;
            bytes = &bytes[take..];
            if self.pending.len() == PLAINTEXT_CHUNK_BYTES {
                self.seal_pending_chunk()?;
            }
        }
        Ok(original_len)
    }

    fn finish(&mut self) -> std::io::Result<()> {
        if self.poisoned || self.writer_active {
            return Err(std::io::Error::other(
                "private audio generation is incomplete or poisoned",
            ));
        }
        if !self.sealed {
            self.seal_pending_chunk()?;
            self.attest_ciphertext_len()?;
            self.sealed = true;
        }
        Ok(())
    }

    fn reset(&mut self) -> std::io::Result<()> {
        if self.writer_active || self.active_readers != 0 {
            return Err(std::io::Error::other("private audio is still in use"));
        }
        self.file.set_len(0)?;
        self.pending.zeroize();
        self.pending.clear();
        self.key_bytes.zeroize();
        getrandom::fill(&mut *self.key_bytes)
            .map_err(|error| std::io::Error::other(format!("audio key failed: {error}")))?;
        getrandom::fill(&mut self.nonce_prefix)
            .map_err(|error| std::io::Error::other(format!("audio nonce failed: {error}")))?;
        getrandom::fill(&mut self.object_id)
            .map_err(|error| std::io::Error::other(format!("audio object id failed: {error}")))?;
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("private audio generation counter overflowed"))?;
        self.plaintext_len = 0;
        self.sealed = false;
        self.poisoned = false;
        self.writer_issued = false;
        Ok(())
    }

    fn decrypt_chunk(&self, chunk_index: u64) -> std::io::Result<Zeroizing<Vec<u8>>> {
        if !self.sealed || self.poisoned {
            return Err(std::io::Error::other(
                "private audio cannot be read before it is sealed",
            ));
        }
        let start = chunk_index
            .checked_mul(PLAINTEXT_CHUNK_BYTES as u64)
            .ok_or_else(|| std::io::Error::other("private audio chunk overflowed"))?;
        if start >= self.plaintext_len {
            return Ok(Zeroizing::new(Vec::new()));
        }
        self.attest_ciphertext_len()?;
        let plaintext_bytes = (self.plaintext_len - start).min(PLAINTEXT_CHUNK_BYTES as u64);
        let mut ciphertext = Zeroizing::new(vec![0u8; PLAINTEXT_CHUNK_BYTES + TAG_BYTES]);
        self.read_ciphertext_at(&mut ciphertext, chunk_index * CIPHERTEXT_CHUNK_BYTES)?;
        let tag = Tag::clone_from_slice(&ciphertext[PLAINTEXT_CHUNK_BYTES..]);
        self.cipher()
            .decrypt_in_place_detached(
                &self.nonce(chunk_index),
                &self.aad(chunk_index),
                &mut ciphertext[..PLAINTEXT_CHUNK_BYTES],
                &tag,
            )
            .map_err(|_| {
                std::io::Error::new(
                    ErrorKind::InvalidData,
                    "private audio authentication failed",
                )
            })?;
        ciphertext.truncate(plaintext_bytes as usize);
        Ok(ciphertext)
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.key_bytes.zeroize();
        self.pending.zeroize();
        let _ = self.file.set_len(0);
    }
}

/// Owned encrypted backing. Clones share only the retained backing handle and
/// in-process key; the filesystem never contains plaintext audio.
#[derive(Clone)]
pub(crate) struct SealedAudio {
    state: Arc<Mutex<State>>,
}

#[derive(Clone)]
pub(crate) struct WeakSealedAudio {
    state: Weak<Mutex<State>>,
}

impl WeakSealedAudio {
    pub(crate) fn upgrade(&self) -> Option<SealedAudio> {
        self.state.upgrade().map(|state| SealedAudio { state })
    }
}

impl SealedAudio {
    pub(crate) fn new_in(root: &Path) -> std::io::Result<Self> {
        let file = tempfile::tempfile_in(root)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            // Linux creates O_TMPFILE descriptors with a requested mode of
            // 0666, so a conventional umask such as 022 leaves an anonymous
            // inode at 0644. Harden the retained descriptor itself before it
            // can hold key-backed state, then keep the strict attestation.
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            let metadata = file.metadata()?;
            if !metadata.is_file()
                || metadata.nlink() != 0
                || metadata.permissions().mode() & 0o077 != 0
            {
                return Err(std::io::Error::other(
                    "sealed private audio backing is not anonymous and owner-private",
                ));
            }
            let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
            if flags < 0 || flags & libc::FD_CLOEXEC == 0 {
                return Err(std::io::Error::other(
                    "sealed private audio backing is unexpectedly inheritable",
                ));
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT};
            if !file.metadata()?.is_file() {
                return Err(std::io::Error::other(
                    "sealed private audio backing is not a regular file",
                ));
            }
            let mut flags = 0_u32;
            let ok = unsafe { GetHandleInformation(file.as_raw_handle() as _, &mut flags) };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            if flags & HANDLE_FLAG_INHERIT != 0 {
                return Err(std::io::Error::other(
                    "sealed private audio backing is unexpectedly inheritable",
                ));
            }
        }
        Ok(Self {
            state: Arc::new(Mutex::new(State::new(file)?)),
        })
    }

    fn lock(&self) -> std::io::Result<MutexGuard<'_, State>> {
        self.state
            .lock()
            .map_err(|_| std::io::Error::other("private audio lock poisoned"))
    }

    pub(crate) fn reset(&self) -> std::io::Result<()> {
        self.lock()?.reset()
    }

    pub(crate) fn writer(&self) -> std::io::Result<SealedAudioWriter> {
        let mut state = self.lock()?;
        if state.writer_issued || state.sealed || state.poisoned {
            return Err(std::io::Error::other(
                "private audio writer lease is unavailable",
            ));
        }
        state.writer_issued = true;
        state.writer_active = true;
        Ok(SealedAudioWriter {
            audio: self.clone(),
        })
    }

    pub(crate) fn finish(&self) -> std::io::Result<()> {
        self.lock()?.finish()
    }

    pub(crate) fn len(&self) -> std::io::Result<u64> {
        Ok(self.lock()?.plaintext_len)
    }

    pub(crate) fn metadata(&self) -> std::io::Result<std::fs::Metadata> {
        self.lock()?.file.metadata()
    }

    pub(crate) fn same_backing(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }

    pub(crate) fn downgrade(&self) -> WeakSealedAudio {
        WeakSealedAudio {
            state: Arc::downgrade(&self.state),
        }
    }

    pub(crate) fn reader(&self) -> std::io::Result<SealedAudioReader> {
        let mut state = self.lock()?;
        if !state.sealed {
            return Err(std::io::Error::other(
                "private audio cannot be read before it is sealed",
            ));
        }
        state.attest_ciphertext_len()?;
        if state.active_readers >= MAX_ACTIVE_READERS {
            return Err(std::io::Error::new(
                ErrorKind::WouldBlock,
                "private audio reader lease limit reached",
            ));
        }
        state.active_readers += 1;
        let len = state.plaintext_len;
        Ok(SealedAudioReader {
            audio: self.clone(),
            position: 0,
            len,
            cached_index: None,
            cached_plaintext: Zeroizing::new(Vec::new()),
        })
    }

    pub(crate) fn verify(&self) -> std::io::Result<()> {
        let state = self.lock()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let metadata = state.file.metadata()?;
            if !metadata.is_file()
                || metadata.nlink() != 0
                || metadata.permissions().mode() & 0o077 != 0
            {
                return Err(std::io::Error::other(
                    "sealed private audio backing lost its anonymous identity",
                ));
            }
        }
        if state.sealed {
            state.attest_ciphertext_len()?;
        }
        Ok(())
    }
}

pub(crate) struct SealedAudioWriter {
    audio: SealedAudio,
}

impl Write for SealedAudioWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.audio.lock()?.append(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // A partial chunk must not be encrypted twice under the same nonce.
        // `SealedAudio::finish` performs the one and only final encryption.
        Ok(())
    }
}

impl Drop for SealedAudioWriter {
    fn drop(&mut self) {
        if let Ok(mut state) = self.audio.state.lock() {
            state.writer_active = false;
        }
    }
}

pub(crate) struct SealedAudioReader {
    audio: SealedAudio,
    position: u64,
    len: u64,
    cached_index: Option<u64>,
    cached_plaintext: Zeroizing<Vec<u8>>,
}

impl SealedAudioReader {
    fn load_chunk(&mut self, chunk_index: u64) -> std::io::Result<()> {
        if self.cached_index != Some(chunk_index) {
            self.cached_plaintext = self.audio.lock()?.decrypt_chunk(chunk_index)?;
            self.cached_index = Some(chunk_index);
        }
        Ok(())
    }
}

impl Drop for SealedAudioReader {
    fn drop(&mut self) {
        if let Ok(mut state) = self.audio.state.lock() {
            state.active_readers = state.active_readers.saturating_sub(1);
        }
    }
}

impl Read for SealedAudioReader {
    fn read(&mut self, mut output: &mut [u8]) -> std::io::Result<usize> {
        let requested = output.len();
        while !output.is_empty() && self.position < self.len {
            let chunk_index = self.position / PLAINTEXT_CHUNK_BYTES as u64;
            let chunk_offset = (self.position % PLAINTEXT_CHUNK_BYTES as u64) as usize;
            self.load_chunk(chunk_index)?;
            let available = &self.cached_plaintext[chunk_offset..];
            let take = available
                .len()
                .min(output.len())
                .min((self.len - self.position) as usize);
            output[..take].copy_from_slice(&available[..take]);
            self.position += take as u64;
            output = &mut output[take..];
        }
        Ok(requested - output.len())
    }
}

impl Seek for SealedAudioReader {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        let next = match position {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.len) + i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
        };
        if !(0..=i128::from(u64::MAX)).contains(&next) {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "private audio seek is outside the capability",
            ));
        }
        self.position = next as u64;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_bytes() -> Vec<u8> {
        (0..(PLAINTEXT_CHUNK_BYTES * 2 + 913))
            .map(|index| ((index * 31 + 17) % 251) as u8)
            .collect()
    }

    fn backing_bytes(audio: &SealedAudio) -> Vec<u8> {
        let state = audio.lock().unwrap();
        let mut bytes = vec![0u8; state.file.metadata().unwrap().len() as usize];
        state.read_ciphertext_at(&mut bytes, 0).unwrap();
        bytes
    }

    fn write_payload(audio: &SealedAudio, bytes: &[u8]) {
        audio.writer().unwrap().write_all(bytes).unwrap();
        audio.finish().unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn conventional_umask_is_hardened_before_private_backing_attestation() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::process::CommandExt;
        const CHILD_ENV: &str = "MINUTES_SEALED_AUDIO_UMASK_CHILD";
        if std::env::var_os(CHILD_ENV).is_some() {
            let dir = tempfile::TempDir::new().unwrap();
            let audio = SealedAudio::new_in(dir.path()).unwrap();
            let mode = audio.metadata().unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            audio.verify().unwrap();
            return;
        }

        let mut command = crate::engine_process::command(std::env::current_exe().unwrap());
        command
            .arg("--exact")
            .arg(
                "sealed_audio::tests::conventional_umask_is_hardened_before_private_backing_attestation",
            )
            .arg("--nocapture")
            .env(CHILD_ENV, "1");
        // SAFETY: this closure runs in the forked child immediately before
        // exec and calls only the async-signal-safe umask syscall.
        unsafe {
            command.pre_exec(|| {
                libc::umask(0o022);
                Ok(())
            });
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "umask-022 child failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    #[cfg(windows)]
    fn windows_ciphertext_backing_handle_is_not_inheritable() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT};

        let dir = tempfile::TempDir::new().unwrap();
        let audio = SealedAudio::new_in(dir.path()).unwrap();
        let state = audio.lock().unwrap();
        let mut flags = 0_u32;
        let ok = unsafe { GetHandleInformation(state.file.as_raw_handle() as _, &mut flags) };
        assert_ne!(ok, 0);
        assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
    }

    #[test]
    fn round_trip_and_independent_seek_cursors() {
        let dir = tempfile::TempDir::new().unwrap();
        let audio = SealedAudio::new_in(dir.path()).unwrap();
        let bytes = synthetic_bytes();
        audio.writer().unwrap().write_all(&bytes).unwrap();
        audio.finish().unwrap();

        let mut first = audio.reader().unwrap();
        let mut second = audio.reader().unwrap();
        let mut prefix = [0u8; 37];
        first.read_exact(&mut prefix).unwrap();
        assert_eq!(&prefix, &bytes[..37]);
        second
            .seek(SeekFrom::Start(PLAINTEXT_CHUNK_BYTES as u64 - 11))
            .unwrap();
        let mut crossing = [0u8; 29];
        second.read_exact(&mut crossing).unwrap();
        assert_eq!(
            &crossing,
            &bytes[PLAINTEXT_CHUNK_BYTES - 11..PLAINTEXT_CHUNK_BYTES + 18]
        );
        let mut remainder = Vec::new();
        first.read_to_end(&mut remainder).unwrap();
        assert_eq!(&remainder, &bytes[37..]);
    }

    #[test]
    fn backing_never_contains_plaintext_and_tampering_fails_closed() {
        let dir = tempfile::TempDir::new().unwrap();
        let audio = SealedAudio::new_in(dir.path()).unwrap();
        let bytes = vec![b'Q'; PLAINTEXT_CHUNK_BYTES + 123];
        audio.writer().unwrap().write_all(&bytes).unwrap();
        audio.finish().unwrap();

        let state = audio.lock().unwrap();
        let mut ciphertext = vec![0u8; state.file.metadata().unwrap().len() as usize];
        state.read_ciphertext_at(&mut ciphertext, 0).unwrap();
        assert!(!ciphertext.windows(128).any(|window| window == [b'Q'; 128]));
        state
            .write_ciphertext_at(&[ciphertext[0] ^ 0x80], 0)
            .unwrap();
        drop(state);

        let mut reader = audio.reader().unwrap();
        let mut output = Vec::new();
        let error = reader.read_to_end(&mut output).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidData);
        assert!(output.is_empty());
    }

    #[test]
    fn reset_rotates_authority_and_reuses_no_nonce() {
        let dir = tempfile::TempDir::new().unwrap();
        let audio = SealedAudio::new_in(dir.path()).unwrap();
        audio
            .writer()
            .unwrap()
            .write_all(b"first synthetic payload")
            .unwrap();
        audio.finish().unwrap();
        let first_key = *audio.lock().unwrap().key_bytes;
        let first_nonce = audio.lock().unwrap().nonce_prefix;

        audio.reset().unwrap();
        audio
            .writer()
            .unwrap()
            .write_all(b"second synthetic payload")
            .unwrap();
        audio.finish().unwrap();
        let state = audio.lock().unwrap();
        assert_ne!(*state.key_bytes, first_key);
        assert_ne!(state.nonce_prefix, first_nonce);
        drop(state);

        let mut reader = audio.reader().unwrap();
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"second synthetic payload");
    }

    #[test]
    fn fixed_slots_hide_exact_length_within_each_chunk_bucket() {
        let dir = tempfile::TempDir::new().unwrap();
        for (plaintext_len, expected_ciphertext_len) in [
            (0, 0),
            (1, CIPHERTEXT_CHUNK_BYTES),
            (PLAINTEXT_CHUNK_BYTES, CIPHERTEXT_CHUNK_BYTES),
            (PLAINTEXT_CHUNK_BYTES + 1, CIPHERTEXT_CHUNK_BYTES * 2),
        ] {
            let audio = SealedAudio::new_in(dir.path()).unwrap();
            write_payload(&audio, &vec![0x5a; plaintext_len]);
            assert_eq!(
                audio.lock().unwrap().file.metadata().unwrap().len(),
                expected_ciphertext_len
            );
        }
    }

    #[test]
    fn writer_and_reader_leases_enforce_generation_boundaries() {
        let dir = tempfile::TempDir::new().unwrap();
        let audio = SealedAudio::new_in(dir.path()).unwrap();
        let mut writer = audio.writer().unwrap();
        assert!(audio.writer().is_err());
        assert!(audio.reader().is_err());
        assert!(audio.reset().is_err());
        writer.write_all(b"lease-bound synthetic payload").unwrap();
        drop(writer);
        audio.finish().unwrap();

        let reader = audio.reader().unwrap();
        assert!(audio.reset().is_err());
        drop(reader);
        audio.reset().unwrap();
        assert_eq!(audio.len().unwrap(), 0);
        assert!(audio.reader().is_err());
        write_payload(&audio, b"next generation");
    }

    #[test]
    fn reader_leases_are_explicitly_bounded() {
        let dir = tempfile::TempDir::new().unwrap();
        let audio = SealedAudio::new_in(dir.path()).unwrap();
        write_payload(&audio, b"bounded independent reader payload");

        let mut readers = (0..MAX_ACTIVE_READERS)
            .map(|_| audio.reader().unwrap())
            .collect::<Vec<_>>();
        let error = match audio.reader() {
            Ok(_) => panic!("reader count must remain resource-bounded"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), ErrorKind::WouldBlock);

        readers.pop();
        assert!(audio.reader().is_ok());
    }

    #[test]
    fn output_budget_failure_poisoning_requires_a_full_reset() {
        let dir = tempfile::TempDir::new().unwrap();
        let audio = SealedAudio::new_in(dir.path()).unwrap();
        let mut writer = audio.writer().unwrap();
        audio.lock().unwrap().plaintext_len = MAX_PLAINTEXT_BYTES;
        let error = writer.write_all(&[0x01]).unwrap_err();
        assert!(error.to_string().contains("resource budget"));
        drop(writer);
        assert!(audio.finish().is_err());
        assert!(audio.reader().is_err());

        audio.reset().unwrap();
        write_payload(&audio, b"bounded recovery");
        let mut reader = audio.reader().unwrap();
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"bounded recovery");
    }

    #[test]
    fn ciphertext_length_extension_and_truncation_fail_closed() {
        let dir = tempfile::TempDir::new().unwrap();
        for delta in [-1_i64, 1] {
            let audio = SealedAudio::new_in(dir.path()).unwrap();
            write_payload(&audio, b"synthetic length attestation payload");
            let state = audio.lock().unwrap();
            let original_len = state.file.metadata().unwrap().len();
            state
                .file
                .set_len((original_len as i64 + delta) as u64)
                .unwrap();
            drop(state);
            let error = match audio.reader() {
                Ok(_) => panic!("changed ciphertext length must fail closed"),
                Err(error) => error,
            };
            assert_eq!(error.kind(), ErrorKind::InvalidData);
        }
    }

    #[test]
    fn chunk_reordering_and_cross_object_copy_fail_authentication() {
        let dir = tempfile::TempDir::new().unwrap();
        let first = SealedAudio::new_in(dir.path()).unwrap();
        let second = SealedAudio::new_in(dir.path()).unwrap();
        let mut first_payload = vec![0x11; PLAINTEXT_CHUNK_BYTES];
        first_payload.extend(vec![0x22; PLAINTEXT_CHUNK_BYTES]);
        let second_payload = vec![0x33; PLAINTEXT_CHUNK_BYTES * 2];
        write_payload(&first, &first_payload);
        write_payload(&second, &second_payload);

        let original_first = backing_bytes(&first);
        {
            let state = first.lock().unwrap();
            state
                .write_ciphertext_at(&original_first[CIPHERTEXT_CHUNK_BYTES as usize..], 0)
                .unwrap();
            state
                .write_ciphertext_at(
                    &original_first[..CIPHERTEXT_CHUNK_BYTES as usize],
                    CIPHERTEXT_CHUNK_BYTES,
                )
                .unwrap();
        }
        let mut reader = first.reader().unwrap();
        let mut output = Vec::new();
        assert_eq!(
            reader.read_to_end(&mut output).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
        drop(reader);

        {
            let state = first.lock().unwrap();
            state
                .write_ciphertext_at(&backing_bytes(&second), 0)
                .unwrap();
        }
        let mut reader = first.reader().unwrap();
        output.clear();
        assert_eq!(
            reader.read_to_end(&mut output).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
    }

    #[test]
    fn ciphertext_from_a_retired_generation_cannot_be_replayed() {
        let dir = tempfile::TempDir::new().unwrap();
        let audio = SealedAudio::new_in(dir.path()).unwrap();
        let payload = b"same-sized generation payload";
        write_payload(&audio, payload);
        let retired_ciphertext = backing_bytes(&audio);

        audio.reset().unwrap();
        write_payload(&audio, &vec![0x44; payload.len()]);
        audio
            .lock()
            .unwrap()
            .write_ciphertext_at(&retired_ciphertext, 0)
            .unwrap();
        let mut reader = audio.reader().unwrap();
        let mut output = Vec::new();
        assert_eq!(
            reader.read_to_end(&mut output).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
    }
}
