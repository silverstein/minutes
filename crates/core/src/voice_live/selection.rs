//! Explicit point-and-talk capture. No background collector, clipboard, clicks,
//! fallback screenshots, or permission prompts. The host reviews the snapshot.
use crate::live_sidekick::work::{Focus, Sensitivity, SourceRef};
use sha2::{Digest, Sha256};

pub fn content_version(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

pub fn from_text(text: &str, origin: &str) -> Result<Focus, String> {
    if text.trim().is_empty() || text.len() > 16_384 || text.contains('\0') {
        return Err("Select or paste between 1 and 16384 bytes of text.".into());
    }
    if origin.is_empty() || origin.len() > 512 || origin.contains('\0') {
        return Err("Invalid selection origin".into());
    }
    let digest = content_version(text);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis()
        .min(u64::MAX as u128) as u64;
    Ok(Focus {
        source: SourceRef {
            id: format!("selection-{}", &digest[..24]),
            locator: format!("selection:{origin}"),
            version: digest,
            observed_at_ms: now,
            sensitivity: Sensitivity::Normal,
        },
        selection: text.into(),
    })
}

pub fn capture() -> Result<Focus, String> {
    #[cfg(target_os = "macos")]
    {
        macos::capture()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Native selection capture is not available on this platform. Paste selected text into the Work window instead.".into())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::ffi::{c_char, c_void, CStr, CString};
    use std::ptr;
    type Ref = *const c_void;
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXUIElementCreateSystemWide() -> Ref;
        fn AXUIElementGetTypeID() -> usize;
        fn AXUIElementCopyAttributeValue(element: Ref, name: Ref, value: *mut Ref) -> i32;
        fn AXUIElementSetMessagingTimeout(element: Ref, seconds: f32) -> i32;
        fn AXUIElementGetPid(element: Ref, pid: *mut i32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(value: Ref);
        fn CFGetTypeID(value: Ref) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFEqual(a: Ref, b: Ref) -> bool;
        fn CFStringCreateWithCString(alloc: Ref, value: *const c_char, encoding: u32) -> Ref;
        fn CFStringGetCString(value: Ref, buffer: *mut c_char, size: isize, encoding: u32) -> bool;
    }
    struct Owned(Ref);
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe { CFRelease(self.0) }
        }
    }
    fn attribute(element: Ref, name: &str) -> Result<Owned, String> {
        let key = CString::new(name).map_err(|e| e.to_string())?;
        let key = unsafe { CFStringCreateWithCString(ptr::null(), key.as_ptr(), 0x08000100) };
        if key.is_null() {
            return Err("Unable to allocate accessibility attribute".into());
        }
        let key = Owned(key);
        let mut value = ptr::null();
        let error = unsafe { AXUIElementCopyAttributeValue(element, key.0, &mut value) };
        if error != 0 || value.is_null() {
            if !value.is_null() {
                unsafe { CFRelease(value) };
            }
            return Err(format!("The active app did not provide {name} (AX {error}). Select text in a supported app or paste it manually."));
        }
        Ok(Owned(value))
    }
    fn string(element: Ref, name: &str) -> Result<String, String> {
        let value = attribute(element, name)?;
        if unsafe { CFGetTypeID(value.0) } != unsafe { CFStringGetTypeID() } {
            return Err("Accessibility attribute was not text".into());
        }
        let mut buffer = vec![0 as c_char; 16_385];
        if !unsafe {
            CFStringGetCString(
                value.0,
                buffer.as_mut_ptr(),
                buffer.len() as isize,
                0x08000100,
            )
        } {
            return Err("Selected text is unreadable or exceeds 16 KB".into());
        }
        unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_str()
            .map(str::to_owned)
            .map_err(|e| e.to_string())
    }
    fn focused(system: Ref) -> Result<Owned, String> {
        let element = attribute(system, "AXFocusedUIElement")?;
        if unsafe { CFGetTypeID(element.0) } != unsafe { AXUIElementGetTypeID() } {
            return Err("Focused object is not an accessibility element".into());
        }
        unsafe {
            AXUIElementSetMessagingTimeout(element.0, 0.5);
        }
        Ok(element)
    }
    pub fn capture() -> Result<Focus, String> {
        if !unsafe { AXIsProcessTrusted() } {
            return Err("Enable Accessibility for Minutes Dev (or your installed Minutes app), then try again. Nothing was captured.".into());
        }
        let raw = unsafe { AXUIElementCreateSystemWide() };
        if raw.is_null() {
            return Err("Accessibility is unavailable".into());
        }
        let system = Owned(raw);
        unsafe {
            AXUIElementSetMessagingTimeout(system.0, 0.5);
        }
        let element = focused(system.0)?;
        let mut pid = 0;
        if unsafe { AXUIElementGetPid(element.0, &mut pid) } != 0
            || pid <= 0
            || pid as u32 == std::process::id()
        {
            return Err("Select text in another app and use the Work shortcut; the Minutes window is not a source.".into());
        }
        // Role must be readable, subrole may be absent. Secure controls are never read.
        let role = string(element.0, "AXRole")?;
        let subrole = string(element.0, "AXSubrole").unwrap_or_default();
        if role.to_ascii_lowercase().contains("secure")
            || subrole.to_ascii_lowercase().contains("secure")
        {
            return Err("Password fields cannot be shared as work context".into());
        }
        let text = string(element.0, "AXSelectedText")?;
        let after = focused(system.0)?;
        if !unsafe { CFEqual(element.0, after.0) } {
            return Err(
                "Focus changed during capture. No mixed or stale selection was accepted.".into(),
            );
        }
        let repeated = string(after.0, "AXSelectedText")?;
        if text != repeated {
            return Err("Selection changed during capture; please try again".into());
        }
        from_text(&text, &format!("macos-pid-{pid}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manual_selection_is_a_versioned_copy_not_an_execution_target() {
        let selected = from_text("The selected paragraph", "manual").unwrap();
        assert_eq!(selected.selection, "The selected paragraph");
        assert_eq!(selected.source.locator, "selection:manual");
        assert_ne!(
            selected.source.version,
            from_text("Changed paragraph", "manual")
                .unwrap()
                .source
                .version
        );
    }
    #[test]
    fn bounded_and_empty_selection_is_rejected() {
        assert!(from_text("", "manual").is_err());
        assert!(from_text(&"x".repeat(16_385), "manual").is_err());
        assert!(from_text("a\0b", "manual").is_err());
    }
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unsupported_platform_does_not_fabricate_capture() {
        assert!(capture().is_err());
    }
}
