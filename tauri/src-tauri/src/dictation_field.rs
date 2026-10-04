//! Ephemeral, opt-in AX cursor context. Never serialized, persisted or sent to a model.
use crate::text_insertion::ActiveTargetContext;
use minutes_core::dictation_experience::CursorContext;

#[derive(Clone)]
pub struct FieldSnapshot {
    pub context: CursorContext,
    pub site: Option<String>,
    #[cfg(target_os = "macos")]
    native: native::Snapshot,
}
pub fn capture(target: Option<&ActiveTargetContext>) -> Option<FieldSnapshot> {
    #[cfg(target_os = "macos")]
    {
        native::capture(target)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = target;
        None
    }
}
impl FieldSnapshot {
    pub fn unchanged(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.native.unchanged()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
}

#[derive(Clone)]
pub struct UndoInsertion {
    before: FieldSnapshot,
    after: FieldSnapshot,
    inserted: String,
}
impl FieldSnapshot {
    pub fn delivered(&self, inserted: &str) -> Option<UndoInsertion> {
        #[cfg(target_os = "macos")]
        {
            let after = FieldSnapshot {
                native: self.native.delivery(inserted)?,
                ..self.clone()
            };
            Some(UndoInsertion {
                before: self.clone(),
                after,
                inserted: inserted.into(),
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = inserted;
            None
        }
    }
}
impl UndoInsertion {
    pub fn process_id(&self) -> Option<i32> {
        #[cfg(target_os = "macos")]
        {
            Some(self.before.native.pid)
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }
    pub fn undo(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.before.native.undo(&self.after.native, &self.inserted)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (&self.before, &self.after, &self.inserted);
            false
        }
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use std::{
        ffi::{c_char, c_void, CString},
        ptr,
        sync::Arc,
    };
    type Ref = *const c_void;
    #[derive(Clone, Copy, PartialEq, Eq)]
    #[repr(C)]
    struct Range {
        location: isize,
        length: isize,
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateSystemWide() -> Ref;
        fn AXUIElementCopyAttributeValue(element: Ref, attr: Ref, value: *mut Ref) -> i32;
        fn AXUIElementGetPid(element: Ref, pid: *mut i32) -> i32;
        fn AXUIElementSetMessagingTimeout(element: Ref, timeout: f32) -> i32;
        fn AXValueGetValue(value: Ref, kind: i32, out: *mut c_void) -> bool;
        fn AXValueGetType(value: Ref) -> i32;
        fn AXValueGetTypeID() -> usize;
        fn AXValueCreate(kind: i32, value: *const c_void) -> Ref;
        fn AXUIElementSetAttributeValue(element: Ref, attr: Ref, value: Ref) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(value: Ref);
        fn CFEqual(a: Ref, b: Ref) -> u8;
        fn CFGetTypeID(value: Ref) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFBooleanGetValue(value: Ref) -> u8;
        fn CFStringCreateWithCString(alloc: Ref, value: *const c_char, encoding: u32) -> Ref;
        fn CFStringGetCString(value: Ref, out: *mut c_char, len: isize, encoding: u32) -> u8;
    }
    struct Owned(Ref);
    // AXUIElement is a retained CF reference to an IPC object, not an NSView.
    // It is immutable here and CF reference counting supports cross-thread use.
    unsafe impl Send for Owned {}
    unsafe impl Sync for Owned {}
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe {
                CFRelease(self.0);
            }
        }
    }
    fn attr(element: Ref, name: &str) -> Option<Owned> {
        let name = CString::new(name).ok()?;
        let key = unsafe { CFStringCreateWithCString(ptr::null(), name.as_ptr(), 0x0800_0100) };
        if key.is_null() {
            return None;
        }
        let key = Owned(key);
        let mut value = ptr::null();
        let result = unsafe { AXUIElementCopyAttributeValue(element, key.0, &mut value) };
        if result == 0 && !value.is_null() {
            Some(Owned(value))
        } else {
            if !value.is_null() {
                drop(Owned(value));
            }
            None
        }
    }
    fn text(element: Ref, name: &str) -> Option<String> {
        let value = attr(element, name)?;
        if unsafe { CFGetTypeID(value.0) != CFStringGetTypeID() } {
            return None;
        }
        // A large document or malformed AX object fails closed; never truncate
        // the field value and then mistake that truncation for the full field.
        let mut bytes = vec![0i8; 32768];
        if unsafe {
            CFStringGetCString(
                value.0,
                bytes.as_mut_ptr(),
                bytes.len() as isize,
                0x0800_0100,
            )
        } == 0
        {
            return None;
        }
        let end = bytes.iter().position(|b| *b == 0)?;
        String::from_utf8(bytes[..end].iter().map(|b| *b as u8).collect()).ok()
    }
    fn focused() -> Option<Owned> {
        let system = unsafe { AXUIElementCreateSystemWide() };
        if system.is_null() {
            return None;
        }
        let system = Owned(system);
        unsafe {
            AXUIElementSetMessagingTimeout(system.0, 0.1);
        }
        let field = attr(system.0, "AXFocusedUIElement")?;
        unsafe {
            AXUIElementSetMessagingTimeout(field.0, 0.1);
        }
        Some(field)
    }
    fn range(field: Ref) -> Option<Range> {
        let value = attr(field, "AXSelectedTextRange")?;
        if unsafe { CFGetTypeID(value.0) != AXValueGetTypeID() || AXValueGetType(value.0) != 4 } {
            return None;
        }
        let mut range = Range {
            location: 0,
            length: 0,
        };
        if unsafe { AXValueGetValue(value.0, 4, (&mut range as *mut Range).cast()) }
            && range.location >= 0
            && range.length >= 0
        {
            Some(range)
        } else {
            None
        }
    }
    #[derive(Clone)]
    pub struct Snapshot {
        element: Arc<Owned>,
        pub(super) pid: i32,
        value: String,
        range: Range,
    }
    impl Snapshot {
        pub fn unchanged(&self) -> bool {
            let Some(field) = focused() else {
                return false;
            };
            let mut pid = 0;
            unsafe {
                AXUIElementGetPid(field.0, &mut pid);
            }
            pid == self.pid
                && unsafe { CFEqual(field.0, self.element.0) } != 0
                && range(field.0) == Some(self.range)
                && text(field.0, "AXValue").as_deref() == Some(&self.value)
        }
    }
    impl Snapshot {
        pub fn delivery(&self, inserted: &str) -> Option<Self> {
            let utf16: Vec<u16> = self.value.encode_utf16().collect();
            let start = self.range.location as usize;
            let end = start + self.range.length as usize;
            let mut expected = utf16[..start].to_vec();
            expected.extend(inserted.encode_utf16());
            expected.extend_from_slice(&utf16[end..]);
            let current = focused()?;
            if unsafe { CFEqual(current.0, self.element.0) } == 0 {
                return None;
            }
            let value = String::from_utf16(&expected).ok()?;
            let after = Snapshot {
                element: self.element.clone(),
                pid: self.pid,
                value,
                range: Range {
                    location: self.range.location + inserted.encode_utf16().count() as isize,
                    length: 0,
                },
            };
            after.unchanged().then_some(after)
        }
        pub fn undo(&self, after: &Self, inserted: &str) -> bool {
            if !after.unchanged() {
                return false;
            }
            let undo_range = Range {
                location: self.range.location,
                length: inserted.encode_utf16().count() as isize,
            };
            let raw_range = unsafe { AXValueCreate(4, (&undo_range as *const Range).cast()) };
            if raw_range.is_null() {
                return false;
            }
            let raw_range = Owned(raw_range);
            let set = |name: &str, value: Ref| -> bool {
                let Ok(name) = CString::new(name) else {
                    return false;
                };
                let key =
                    unsafe { CFStringCreateWithCString(ptr::null(), name.as_ptr(), 0x0800_0100) };
                if key.is_null() {
                    return false;
                }
                let key = Owned(key);
                unsafe { AXUIElementSetAttributeValue(self.element.0, key.0, value) == 0 }
            };
            let before: Vec<u16> = self.value.encode_utf16().collect();
            let start = self.range.location as usize;
            let selected =
                String::from_utf16(&before[start..start + self.range.length as usize]).ok();
            let Some(selected) = selected.and_then(|v| CString::new(v).ok()) else {
                return false;
            };
            let value =
                unsafe { CFStringCreateWithCString(ptr::null(), selected.as_ptr(), 0x0800_0100) };
            if value.is_null() {
                return false;
            }
            let value = Owned(value);
            if !set("AXSelectedTextRange", raw_range.0) {
                return false;
            }
            if !set("AXSelectedText", value.0) {
                let restored = unsafe { AXValueCreate(4, (&after.range as *const Range).cast()) };
                if !restored.is_null() {
                    let restored = Owned(restored);
                    set("AXSelectedTextRange", restored.0);
                }
                return false;
            }
            if text(self.element.0, "AXValue").as_deref() != Some(&self.value) {
                return false;
            }
            let original_range = unsafe { AXValueCreate(4, (&self.range as *const Range).cast()) };
            if original_range.is_null() {
                return false;
            }
            let original_range = Owned(original_range);
            set("AXSelectedTextRange", original_range.0) && self.unchanged()
        }
    }

    pub fn capture(target: Option<&ActiveTargetContext>) -> Option<FieldSnapshot> {
        let target = target?;
        let identity = format!(
            "{} {}",
            target.app_name.as_deref().unwrap_or(""),
            target.bundle_id.as_deref().unwrap_or("")
        )
        .to_lowercase();
        // Password managers and terminal buffers are never inspected. Terminal
        // prose remains available through explicit app preferences.
        if [
            "1password",
            "bitwarden",
            "keychain",
            "password",
            "bank",
            "ghostty",
            "terminal",
            "iterm",
            "warp",
            "wezterm",
            "kitty",
            "alacritty",
            "com.useminutes",
        ]
        .iter()
        .any(|s| identity.contains(s))
        {
            return None;
        }
        let browser = [
            "safari", "chrome", "chromium", "firefox", "brave", "arc", "edge",
        ]
        .iter()
        .any(|s| identity.contains(s));
        let started = std::time::Instant::now();
        let field = focused()?;
        let mut pid = 0;
        if unsafe { AXUIElementGetPid(field.0, &mut pid) } != 0 || target.process_id != Some(pid) {
            return None;
        }
        let role = text(field.0, "AXRole")?;
        if !["AXTextField", "AXTextArea"].contains(&role.as_str())
            || (browser && role != "AXTextArea")
        {
            return None;
        }
        if text(field.0, "AXSubrole").is_some_and(|v| v.to_lowercase().contains("secure")) {
            return None;
        }
        if text(field.0, "AXDescription").is_some_and(|v| v.to_lowercase().contains("password")) {
            return None;
        }
        if attr(field.0, "AXProtectedContent").is_some_and(|v| unsafe {
            CFGetTypeID(v.0) == CFBooleanGetTypeID() && CFBooleanGetValue(v.0) != 0
        }) {
            return None;
        }
        let site = if browser {
            attr(field.0, "AXWindow")
                .and_then(|window| {
                    unsafe {
                        AXUIElementSetMessagingTimeout(window.0, 0.1);
                    }
                    text(window.0, "AXDocument")
                })
                .and_then(|url| reqwest::Url::parse(&url).ok())
                .and_then(|url| {
                    if !["https", "http"].contains(&url.scheme()) {
                        return None;
                    }
                    url.host_str().map(|host| host.to_lowercase())
                })
        } else {
            None
        };
        if browser
            && (site.is_none()
                || site.as_ref().is_some_and(|site| {
                    [
                        "bank",
                        "chase",
                        "schwab",
                        "fidelity",
                        "vanguard",
                        "robinhood",
                        "paypal",
                        "1password",
                        "bitwarden",
                    ]
                    .iter()
                    .any(|word| site.contains(word))
                }))
        {
            return None;
        }
        let range = range(field.0)?;
        let value = text(field.0, "AXValue")?;
        let utf16: Vec<u16> = value.encode_utf16().collect();
        let start = usize::try_from(range.location).ok()?;
        let end = start.checked_add(usize::try_from(range.length).ok()?)?;
        if end > utf16.len() || range.length > 256 {
            return None;
        }
        let before = String::from_utf16(&utf16[..start])
            .ok()?
            .chars()
            .rev()
            .take(256)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        let after = String::from_utf16(&utf16[end..])
            .ok()?
            .chars()
            .take(256)
            .collect();
        let selected = String::from_utf16(&utf16[start..end]).ok()?;
        if started.elapsed().as_millis() > 150 {
            return None;
        }
        Some(FieldSnapshot {
            context: CursorContext {
                before,
                selected,
                after,
            },
            site,
            native: Snapshot {
                element: Arc::new(field),
                pid,
                value,
                range,
            },
        })
    }
}
