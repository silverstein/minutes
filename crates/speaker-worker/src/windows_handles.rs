//! Retire ambient Windows handles before models or audio are opened.
//! Adapted from the existing Minutes graph worker fence.
pub fn close_inherited_handles() -> Result<(), String> {
    use std::ffi::c_void;
    use std::mem::size_of;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Console::GetStdHandle;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    const SYSTEM_EXTENDED_HANDLE_INFORMATION: u32 = 64;
    const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;
    const MAX_HANDLE_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
    const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SystemHandleEntry {
        object: *mut c_void,
        process_id: usize,
        handle_value: usize,
        granted_access: u32,
        creator_backtrace_index: u16,
        object_type_index: u16,
        handle_attributes: u32,
        reserved: u32,
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQuerySystemInformation(
            information_class: u32,
            information: *mut c_void,
            information_bytes: u32,
            returned_bytes: *mut u32,
        ) -> i32;
    }

    let mut capacity = 64 * 1024usize;
    let (storage, returned) = loop {
        if capacity > MAX_HANDLE_SNAPSHOT_BYTES {
            return Err("speaker worker inherited-handle inventory exceeded its budget".into());
        }
        let words = capacity.div_ceil(size_of::<usize>());
        let mut storage = vec![0usize; words];
        let mut returned = 0u32;
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_EXTENDED_HANDLE_INFORMATION,
                storage.as_mut_ptr().cast(),
                (words * size_of::<usize>()) as u32,
                &mut returned,
            )
        };
        if status == 0 {
            break (storage, returned as usize);
        }
        if status != STATUS_INFO_LENGTH_MISMATCH {
            return Err("speaker worker inherited handles could not be inventoried".into());
        }
        capacity = capacity
            .saturating_mul(2)
            .max(returned as usize)
            .min(MAX_HANDLE_SNAPSHOT_BYTES + 1);
    };

    let header_bytes = size_of::<usize>() * 2;
    if returned < header_bytes {
        return Err("speaker worker inherited-handle inventory was malformed".into());
    }
    let base = storage.as_ptr().cast::<u8>();
    let count = unsafe { std::ptr::read_unaligned(base.cast::<usize>()) };
    let entries_bytes = count
        .checked_mul(size_of::<SystemHandleEntry>())
        .and_then(|bytes| bytes.checked_add(header_bytes))
        .ok_or_else(|| "speaker worker inherited-handle inventory overflowed".to_string())?;
    if entries_bytes > returned || entries_bytes > storage.len() * size_of::<usize>() {
        return Err("speaker worker inherited-handle inventory was truncated".into());
    }

    let preserved = unsafe {
        [
            GetStdHandle(STD_INPUT_HANDLE),
            GetStdHandle(STD_OUTPUT_HANDLE),
            GetStdHandle(STD_ERROR_HANDLE),
        ]
    };
    let process_id = unsafe { GetCurrentProcessId() } as usize;
    for index in 0..count {
        let offset = header_bytes + index * size_of::<SystemHandleEntry>();
        let entry =
            unsafe { std::ptr::read_unaligned(base.add(offset).cast::<SystemHandleEntry>()) };
        if entry.process_id != process_id {
            continue;
        }
        let handle = entry.handle_value as HANDLE;
        if handle.is_null() || handle == INVALID_HANDLE_VALUE || preserved.contains(&handle) {
            continue;
        }
        // DLL and Rust startup run before main and can own non-inheritable
        // handles already. They are not parent capabilities. Closing every
        // process handle can corrupt those runtimes before inference starts.
        // Non-stdio inheritance requires HANDLE_FLAG_INHERIT; retain the
        // runtime's non-inheritable handles and retire every inheritable one.
        let mut flags = 0u32;
        if unsafe { GetHandleInformation(handle, &mut flags) } == 0 {
            return Err("speaker worker handle flags could not be inspected".into());
        }
        if flags & HANDLE_FLAG_INHERIT == 0 {
            continue;
        }
        if unsafe { CloseHandle(handle) } == 0 {
            return Err("speaker worker could not retire an inherited handle".into());
        }
    }
    if let Some(value) = std::env::var_os("MINUTES_SPEAKER_TEST_INHERITED_HANDLE") {
        std::env::remove_var("MINUTES_SPEAKER_TEST_INHERITED_HANDLE");
        let value = value
            .to_str()
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(|| "speaker worker inherited-handle canary was malformed".to_string())?;
        let mut flags = 0u32;
        if unsafe { GetHandleInformation(value as HANDLE, &mut flags) } != 0 {
            return Err("speaker worker retained an ambient inherited handle".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{
        GetHandleInformation, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcessId, GetProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    fn process_handle(inheritable: bool) -> OwnedHandle {
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION,
                i32::from(inheritable),
                GetCurrentProcessId(),
            )
        };
        assert!(!handle.is_null());
        // SAFETY: OpenProcess just returned a new, owned process handle.
        unsafe { OwnedHandle::from_raw_handle(handle) }
    }

    #[test]
    fn runtime_handles_survive_while_ambient_parent_handle_is_retired() {
        let ambient = process_handle(true);
        let file = tempfile::NamedTempFile::new().unwrap();
        assert_ne!(
            unsafe {
                SetHandleInformation(
                    file.as_file().as_raw_handle(),
                    HANDLE_FLAG_INHERIT,
                    HANDLE_FLAG_INHERIT,
                )
            },
            0
        );
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "windows_handles::tests::fence_child",
                "--test-threads=1",
                "--nocapture",
            ])
            .env("MINUTES_SPEAKER_FENCE_TEST_CHILD", "1")
            .env(
                "MINUTES_SPEAKER_TEST_INHERITED_HANDLE",
                (ambient.as_raw_handle() as usize).to_string(),
            )
            .env(
                "MINUTES_SPEAKER_FENCE_TEST_FILE_HANDLE",
                (file.as_file().as_raw_handle() as usize).to_string(),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "isolated handle regression failed: {status}"
                );
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("isolated handle regression exceeded its deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn fence_child() {
        if std::env::var("MINUTES_SPEAKER_FENCE_TEST_CHILD").as_deref() != Ok("1") {
            return;
        }
        let ambient = std::env::var("MINUTES_SPEAKER_TEST_INHERITED_HANDLE")
            .unwrap()
            .parse::<usize>()
            .unwrap() as HANDLE;
        let mut flags = 0u32;
        assert_ne!(unsafe { GetHandleInformation(ambient, &mut flags) }, 0);
        assert_ne!(flags & HANDLE_FLAG_INHERIT, 0, "inheritance precondition");
        assert_ne!(unsafe { GetProcessId(ambient) }, 0);
        let file = std::env::var("MINUTES_SPEAKER_FENCE_TEST_FILE_HANDLE")
            .unwrap()
            .parse::<usize>()
            .unwrap() as HANDLE;
        assert_ne!(unsafe { GetHandleInformation(file, &mut flags) }, 0);
        assert_ne!(
            flags & HANDLE_FLAG_INHERIT,
            0,
            "file inheritance precondition"
        );

        let runtime = process_handle(false);
        assert_ne!(
            unsafe { GetHandleInformation(runtime.as_raw_handle(), &mut flags) },
            0
        );
        assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
        super::close_inherited_handles().unwrap();
        assert_eq!(
            unsafe { GetProcessId(runtime.as_raw_handle()) },
            unsafe { GetCurrentProcessId() },
            "runtime-owned handle was closed"
        );
        assert_eq!(unsafe { GetHandleInformation(ambient, &mut flags) }, 0);
        assert_eq!(unsafe { GetHandleInformation(file, &mut flags) }, 0);
    }
}
