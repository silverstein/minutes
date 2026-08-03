//! Authenticated macOS XPC transport for policy graph projection.
//!
//! A suspended spawn is not an authenticity primitive: another process with
//! the same uid can resume the child before the parent attests it. This
//! transport delegates launch to the embedded XPC
//! service and installs an exact code-signing requirement on the connection
//! before the first content-bearing message. The service independently
//! authenticates its parent. Only after a content-free `begin` round trip
//! succeeds do framed meeting bytes cross the connection.

use block2::{Block, RcBlock};
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::{Duration, Instant};

type XpcObject = *mut c_void;
type PeerRequirementFn = unsafe extern "C" fn(XpcObject, *const c_char) -> c_int;
type XpcConnectionHandler = unsafe extern "C" fn(XpcObject);

const GRAPH_SUBSYSTEM: &str = "policy graph";
const APPLE_SPEECH_SUBSYSTEM: &str = "Apple Speech";
const XPC_SERVICE_NAME: &[u8] = b"com.useminutes.graph-worker\0";
const APPLE_SPEECH_XPC_SERVICE_NAME: &[u8] = b"com.useminutes.apple-speech-worker\0";
const COMMAND_KEY: &[u8] = b"command\0";
const SEQUENCE_KEY: &[u8] = b"sequence\0";
const OFFSET_KEY: &[u8] = b"offset\0";
const DATA_KEY: &[u8] = b"data\0";
const OK_KEY: &[u8] = b"ok\0";
const BUSY_KEY: &[u8] = b"busy\0";
const TERMINAL_KEY: &[u8] = b"terminal\0";
const SERVICE_NONCE_KEY: &[u8] = b"service_nonce\0";
const LENGTH_KEY: &[u8] = b"length\0";
const COMMAND_BEGIN: &[u8] = b"begin\0";
const COMMAND_CHUNK: &[u8] = b"chunk\0";
const COMMAND_FINISH: &[u8] = b"finish\0";
const COMMAND_PULL: &[u8] = b"pull\0";
const COMMAND_ABORT: &[u8] = b"abort\0";
const XPC_CHUNK_BYTES: usize = 60 * 1024;
const XPC_ERROR_CONNECTION_INTERRUPTED_SYMBOL: &[u8] = b"_xpc_error_connection_interrupted\0";
const XPC_ERROR_CONNECTION_INVALID_SYMBOL: &[u8] = b"_xpc_error_connection_invalid\0";
static XPC_SETTLEMENT_FAILED: AtomicBool = AtomicBool::new(false);
static XPC_PARENT_REQUEST_LOCK: Mutex<()> = Mutex::new(());
static APPLE_SPEECH_XPC_SETTLEMENT_FAILED: AtomicBool = AtomicBool::new(false);
static APPLE_SPEECH_XPC_PARENT_REQUEST_LOCK: Mutex<()> = Mutex::new(());
static XPC_PARENT_CALLBACK_QUEUE: OnceLock<usize> = OnceLock::new();
static GRAPH_SERVICE_NONCE: OnceLock<[u8; 16]> = OnceLock::new();
static GRAPH_SERVICE_CLAIMED: AtomicBool = AtomicBool::new(false);
static APPLE_SPEECH_SERVICE_NONCE: OnceLock<[u8; 16]> = OnceLock::new();
static APPLE_SPEECH_SERVICE_CLAIMED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" {
    fn minutes_current_process_is_trusted_distribution() -> c_int;
    fn minutes_validate_graph_authority_bundle(
        authority_bundle_path: *const c_char,
        current_executable_path: *const c_char,
        running_parent_cdhash: *const u8,
        running_parent_cdhash_len: isize,
    ) -> c_int;
    fn minutes_validate_apple_speech_authority_bundle(
        authority_bundle_path: *const c_char,
        current_executable_path: *const c_char,
        running_parent_cdhash: *const u8,
        running_parent_cdhash_len: isize,
    ) -> c_int;
    fn csops(
        pid: libc::pid_t,
        operation: libc::c_uint,
        user_address: *mut c_void,
        user_size: libc::size_t,
    ) -> c_int;
    fn SecRandomCopyBytes(random: *const c_void, count: usize, bytes: *mut u8) -> c_int;

    fn xpc_connection_create(name: *const c_char, target_queue: *mut c_void) -> XpcObject;
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> *mut c_void;
    fn xpc_connection_set_event_handler(connection: XpcObject, handler: &Block<dyn Fn(XpcObject)>);
    fn xpc_connection_send_message_with_reply(
        connection: XpcObject,
        message: XpcObject,
        reply_queue: *mut c_void,
        handler: &Block<dyn Fn(XpcObject)>,
    );
    fn xpc_connection_send_message(connection: XpcObject, message: XpcObject);
    fn xpc_connection_send_barrier(connection: XpcObject, barrier: &Block<dyn Fn()>);
    fn xpc_connection_resume(connection: XpcObject);
    fn xpc_connection_cancel(connection: XpcObject);
    fn xpc_dictionary_create(
        keys: *const *const c_char,
        values: *const XpcObject,
        count: usize,
    ) -> XpcObject;
    fn xpc_dictionary_create_reply(original: XpcObject) -> XpcObject;
    fn xpc_dictionary_set_string(dictionary: XpcObject, key: *const c_char, value: *const c_char);
    fn xpc_dictionary_get_string(dictionary: XpcObject, key: *const c_char) -> *const c_char;
    fn xpc_dictionary_set_uint64(dictionary: XpcObject, key: *const c_char, value: u64);
    fn xpc_dictionary_get_uint64(dictionary: XpcObject, key: *const c_char) -> u64;
    fn xpc_dictionary_set_bool(dictionary: XpcObject, key: *const c_char, value: bool);
    fn xpc_dictionary_get_bool(dictionary: XpcObject, key: *const c_char) -> bool;
    fn xpc_dictionary_set_data(
        dictionary: XpcObject,
        key: *const c_char,
        bytes: *const c_void,
        length: usize,
    );
    fn xpc_dictionary_get_data(
        dictionary: XpcObject,
        key: *const c_char,
        length: *mut usize,
    ) -> *const c_void;
    fn xpc_get_type(object: XpcObject) -> *const c_void;
    fn xpc_type_get_name(kind: *const c_void) -> *const c_char;
    fn xpc_retain(object: XpcObject) -> XpcObject;
    fn xpc_release(object: XpcObject);
    fn xpc_main(handler: XpcConnectionHandler) -> !;
}

fn parent_callback_queue() -> Result<*mut c_void, String> {
    let queue = *XPC_PARENT_CALLBACK_QUEUE.get_or_init(|| {
        let label = b"com.useminutes.graph-worker.parent\0";
        unsafe { dispatch_queue_create(label.as_ptr().cast(), std::ptr::null()) as usize }
    }) as *mut c_void;
    if queue.is_null() {
        Err("policy graph XPC callback queue could not be created".into())
    } else {
        Ok(queue)
    }
}

fn ensure_transport_available(subsystem: &str, poisoned: &AtomicBool) -> Result<(), String> {
    if poisoned.load(Ordering::Acquire) {
        Err(format!(
            "{subsystem} XPC transport requires an application restart after an unconfirmed service exit"
        ))
    } else {
        Ok(())
    }
}

fn lock_parent_request<'a>(
    subsystem: &str,
    lock: &'a Mutex<()>,
    poisoned: &AtomicBool,
    deadline: Instant,
) -> Result<MutexGuard<'a, ()>, String> {
    loop {
        match lock.try_lock() {
            Ok(guard) => {
                ensure_transport_available(subsystem, poisoned)?;
                return Ok(guard);
            }
            Err(TryLockError::Poisoned(_)) => {
                return Err(format!("{subsystem} XPC parent request lock was poisoned"));
            }
            Err(TryLockError::WouldBlock) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(format!(
                        "{subsystem} XPC parent request admission exceeded its wall-clock budget"
                    ));
                }
                std::thread::sleep(remaining.min(Duration::from_millis(2)));
            }
        }
    }
}

const CS_OPS_CDHASH: libc::c_uint = 5;

pub(crate) fn current_process_is_trusted_distribution() -> bool {
    trusted_distribution_verdict() == TrustedDistribution::Yes
}

/// Whether this process is a trusted distribution build.
///
/// `Indeterminate` is not `No`. The Security evaluation can fail to complete,
/// and treating that as "this is a development build" is what allowed a signed
/// worker to silently install a weaker peer requirement.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TrustedDistribution {
    Yes,
    No,
    Indeterminate,
}

pub(crate) fn trusted_distribution_verdict() -> TrustedDistribution {
    verdict_from_status(unsafe { minutes_current_process_is_trusted_distribution() })
}

fn verdict_from_status(status: c_int) -> TrustedDistribution {
    match status {
        1 => TrustedDistribution::Yes,
        0 => TrustedDistribution::No,
        _ => TrustedDistribution::Indeterminate,
    }
}

pub(crate) fn current_process_cdhash() -> std::io::Result<[u8; 20]> {
    let mut cdhash = [0_u8; 20];
    let status = unsafe {
        csops(
            libc::getpid(),
            CS_OPS_CDHASH,
            cdhash.as_mut_ptr().cast(),
            cdhash.len(),
        )
    };
    if status == 0 {
        Ok(cdhash)
    } else {
        Err(std::io::Error::last_os_error())
    }
}

pub(crate) fn peer_requirement_api_available() -> bool {
    load_peer_requirement().is_some()
}

fn load_peer_requirement() -> Option<PeerRequirementFn> {
    let symbol = b"xpc_connection_set_peer_code_signing_requirement\0";
    let address = unsafe { libc::dlsym(libc::RTLD_DEFAULT, symbol.as_ptr().cast()) };
    (!address.is_null()).then(|| unsafe { std::mem::transmute(address) })
}

fn set_peer_requirement(connection: XpcObject, requirement: &CStr) -> Result<(), String> {
    let set_requirement = load_peer_requirement()
        .ok_or_else(|| "authenticated XPC is unavailable on this macOS version".to_string())?;
    let status = unsafe { set_requirement(connection, requirement.as_ptr()) };
    if status == 0 {
        Ok(())
    } else {
        Err("the XPC code-signing requirement was rejected".into())
    }
}

fn xpc_type_is(object: XpcObject, expected: &str) -> bool {
    if object.is_null() {
        return false;
    }
    let kind = unsafe { xpc_get_type(object) };
    if kind.is_null() {
        return false;
    }
    let name = unsafe { xpc_type_get_name(kind) };
    !name.is_null() && unsafe { CStr::from_ptr(name) }.to_bytes() == expected.as_bytes()
}

fn xpc_is_connection_end(event: XpcObject) -> bool {
    if event.is_null() {
        return false;
    }
    [
        XPC_ERROR_CONNECTION_INTERRUPTED_SYMBOL,
        XPC_ERROR_CONNECTION_INVALID_SYMBOL,
    ]
    .into_iter()
    .any(|symbol| {
        let object = unsafe { libc::dlsym(libc::RTLD_DEFAULT, symbol.as_ptr().cast()) };
        !object.is_null() && event == object
    })
}

fn cstring_path(path: &Path, description: &str) -> Result<CString, String> {
    use std::os::unix::ffi::OsStrExt;
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| format!("{description} contained a NUL byte"))
}

pub(crate) fn validate_authority_bundle(authority_bundle: &Path) -> Result<(), String> {
    let authority_bundle = cstring_path(authority_bundle, "graph worker authority bundle path")?;
    let current_executable =
        std::env::current_exe().map_err(|_| "current executable path was unavailable")?;
    let current_executable = cstring_path(&current_executable, "current executable path")?;
    let running_parent_cdhash =
        current_process_cdhash().map_err(|_| "current executable identity was unavailable")?;
    let status = unsafe {
        minutes_validate_graph_authority_bundle(
            authority_bundle.as_ptr(),
            current_executable.as_ptr(),
            running_parent_cdhash.as_ptr(),
            running_parent_cdhash.len() as isize,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err("the application bundle did not seal the graph worker authority".into())
    }
}

pub(crate) fn validate_apple_speech_authority_bundle(
    authority_bundle: &Path,
) -> Result<(), String> {
    let authority_bundle = cstring_path(
        authority_bundle,
        "Apple Speech worker authority bundle path",
    )?;
    let current_executable =
        std::env::current_exe().map_err(|_| "current executable path was unavailable")?;
    let current_executable = cstring_path(&current_executable, "current executable path")?;
    let running_parent_cdhash =
        current_process_cdhash().map_err(|_| "current executable identity was unavailable")?;
    let status = unsafe {
        minutes_validate_apple_speech_authority_bundle(
            authority_bundle.as_ptr(),
            current_executable.as_ptr(),
            running_parent_cdhash.as_ptr(),
            running_parent_cdhash.len() as isize,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err("the application bundle did not seal the Apple Speech worker authority".into())
    }
}

struct OwnedXpc(XpcObject);

impl OwnedXpc {
    fn dictionary() -> Result<Self, String> {
        let object = unsafe { xpc_dictionary_create(std::ptr::null(), std::ptr::null(), 0) };
        if object.is_null() {
            Err("XPC could not allocate a request".into())
        } else {
            Ok(Self(object))
        }
    }
}

impl Drop for OwnedXpc {
    fn drop(&mut self) {
        unsafe { xpc_release(self.0) };
    }
}

struct Connection {
    subsystem: &'static str,
    object: XpcObject,
    invalidated: mpsc::Receiver<()>,
    service_nonce: Mutex<Option<[u8; 16]>>,
    transport_failed: Arc<AtomicBool>,
    terminal_acknowledged: AtomicBool,
}

impl Connection {
    fn wait_for_service_exit(&self, deadline: Instant) -> Result<(), String> {
        if !self.terminal_acknowledged.load(Ordering::Acquire) {
            return Err(format!(
                "{} XPC terminal settlement was not acknowledged",
                self.subsystem
            ));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "{} XPC service exit exceeded its wall-clock budget",
                self.subsystem
            ));
        }
        self.invalidated.recv_timeout(remaining).map_err(|_| {
            format!(
                "{} XPC service exit exceeded its wall-clock budget",
                self.subsystem
            )
        })
    }

    fn send_with_reply(&self, message: XpcObject, deadline: Instant) -> Result<OwnedXpc, String> {
        if self.transport_failed.load(Ordering::Acquire) {
            return Err(format!(
                "{} XPC transport ended before the next request",
                self.subsystem
            ));
        }
        if let Some(nonce) = *self
            .service_nonce
            .lock()
            .map_err(|_| format!("{} XPC service nonce lock was poisoned", self.subsystem))?
        {
            unsafe {
                xpc_dictionary_set_data(
                    message,
                    SERVICE_NONCE_KEY.as_ptr().cast(),
                    nonce.as_ptr().cast(),
                    nonce.len(),
                );
            }
        }
        let reply = match send_with_reply(
            self.subsystem,
            self.object,
            parent_callback_queue()?,
            message,
            deadline,
        ) {
            Ok(reply) => reply,
            Err(error) => {
                self.transport_failed.store(true, Ordering::Release);
                return Err(error);
            }
        };
        let service_nonce = match service_nonce_from_reply(self.subsystem, reply.0) {
            Ok(nonce) => nonce,
            Err(error) => {
                self.transport_failed.store(true, Ordering::Release);
                return Err(error);
            }
        };
        let mut expected_nonce = self
            .service_nonce
            .lock()
            .map_err(|_| format!("{} XPC service nonce lock was poisoned", self.subsystem))?;
        if let Err(error) = bind_service_nonce(self.subsystem, &mut expected_nonce, service_nonce) {
            self.transport_failed.store(true, Ordering::Release);
            return Err(error);
        }
        let terminal = unsafe { xpc_dictionary_get_bool(reply.0, TERMINAL_KEY.as_ptr().cast()) };
        if terminal {
            self.terminal_acknowledged.store(true, Ordering::Release);
        }
        if self.transport_failed.load(Ordering::Acquire) && !terminal {
            return Err(format!(
                "{} XPC transport ended before a terminal reply",
                self.subsystem
            ));
        }
        Ok(reply)
    }

    fn settle(&self, abort: bool, deadline: Instant) -> Result<(), String> {
        if self.transport_failed.load(Ordering::Acquire)
            && !self.terminal_acknowledged.load(Ordering::Acquire)
        {
            return Err(format!(
                "{} XPC transport failed before terminal acknowledgement",
                self.subsystem
            ));
        }
        if abort && !self.terminal_acknowledged.load(Ordering::Acquire) {
            let message = OwnedXpc::dictionary().map_err(|_| {
                format!("{} XPC terminal abort could not be created", self.subsystem)
            })?;
            set_command(message.0, COMMAND_ABORT);
            let reply = self.send_with_reply(message.0, deadline).map_err(|_| {
                format!("{} XPC terminal abort was not acknowledged", self.subsystem)
            })?;
            if !unsafe { xpc_dictionary_get_bool(reply.0, OK_KEY.as_ptr().cast()) } {
                return Err(format!(
                    "{} XPC terminal abort was rejected",
                    self.subsystem
                ));
            }
        }
        if !self.terminal_acknowledged.load(Ordering::Acquire) {
            return Err(format!(
                "{} XPC terminal settlement was not acknowledged",
                self.subsystem
            ));
        }
        self.wait_for_service_exit(deadline)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        unsafe {
            xpc_connection_cancel(self.object);
            xpc_release(self.object);
        }
    }
}

fn set_command(message: XpcObject, command: &[u8]) {
    unsafe {
        xpc_dictionary_set_string(
            message,
            COMMAND_KEY.as_ptr().cast(),
            command.as_ptr().cast(),
        );
    }
}

fn service_nonce_from_reply(subsystem: &str, reply: XpcObject) -> Result<[u8; 16], String> {
    let mut length = 0_usize;
    let data =
        unsafe { xpc_dictionary_get_data(reply, SERVICE_NONCE_KEY.as_ptr().cast(), &mut length) };
    if data.is_null() || length != 16 {
        return Err(format!(
            "{subsystem} XPC service reply lacked its exact process nonce"
        ));
    }
    let mut nonce = [0_u8; 16];
    nonce.copy_from_slice(unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) });
    Ok(nonce)
}

fn bind_service_nonce(
    subsystem: &str,
    expected: &mut Option<[u8; 16]>,
    observed: [u8; 16],
) -> Result<(), String> {
    match *expected {
        None => *expected = Some(observed),
        Some(current) if current == observed => {}
        Some(_) => {
            return Err(format!(
                "{subsystem} XPC service generation changed mid-request"
            ))
        }
    }
    Ok(())
}

fn send_with_reply(
    subsystem: &str,
    connection: XpcObject,
    reply_queue: *mut c_void,
    message: XpcObject,
    deadline: Instant,
) -> Result<OwnedXpc, String> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(format!(
            "{subsystem} XPC operation exceeded its wall-clock budget"
        ));
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    let handler = RcBlock::new(move |reply: XpcObject| {
        let retained = unsafe { xpc_retain(reply) };
        if sender.send(retained as usize).is_err() {
            unsafe { xpc_release(retained) };
        }
    });
    unsafe {
        xpc_connection_send_message_with_reply(connection, message, reply_queue, &handler);
    }
    let reply = receiver
        .recv_timeout(remaining)
        .map_err(|_| format!("{subsystem} XPC operation exceeded its wall-clock budget"))?;
    let reply = reply as XpcObject;
    if !xpc_type_is(reply, "dictionary") {
        unsafe { xpc_release(reply) };
        return Err(format!(
            "{subsystem} XPC peer was unavailable or unauthenticated"
        ));
    }
    Ok(OwnedXpc(reply))
}

fn open_authenticated_connection(
    exact_cdhash: &[u8; 20],
    trusted_distribution: bool,
    deadline: Instant,
) -> Result<Connection, String> {
    let callback_queue = parent_callback_queue()?;
    let connection =
        unsafe { xpc_connection_create(XPC_SERVICE_NAME.as_ptr().cast(), callback_queue) };
    if connection.is_null() {
        return Err("policy graph XPC service could not be created".into());
    }
    let (invalidated_sender, invalidated) = mpsc::channel();
    let transport_failed = Arc::new(AtomicBool::new(false));
    let connection = Connection {
        subsystem: GRAPH_SUBSYSTEM,
        object: connection,
        invalidated,
        service_nonce: Mutex::new(None),
        transport_failed: Arc::clone(&transport_failed),
        terminal_acknowledged: AtomicBool::new(false),
    };
    let encoded = exact_cdhash
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let mut requirement =
        format!("identifier \"com.useminutes.graph-worker\" and cdhash H\"{encoded}\"");
    if trusted_distribution {
        requirement.push_str(
            " and anchor apple generic and certificate leaf[subject.OU] = \"63TMLKT8HN\"",
        );
    }
    let requirement = CString::new(requirement)
        .map_err(|_| "policy graph XPC requirement was malformed".to_string())?;
    set_peer_requirement(connection.object, &requirement)?;
    let events = RcBlock::new(move |event: XpcObject| {
        if xpc_is_connection_end(event) {
            transport_failed.store(true, Ordering::Release);
            let _ = invalidated_sender.send(());
        }
    });
    unsafe {
        xpc_connection_set_event_handler(connection.object, &events);
        xpc_connection_resume(connection.object);
    }

    // A content-free round trip proves the named service exists and that XPC
    // accepted its exact code requirement before any source frame is sent.
    let begin_result = (|| {
        let begin = OwnedXpc::dictionary()?;
        set_command(begin.0, COMMAND_BEGIN);
        let reply = connection.send_with_reply(begin.0, deadline)?;
        if unsafe { xpc_dictionary_get_bool(reply.0, BUSY_KEY.as_ptr().cast()) } {
            return Ok(false);
        }
        if !unsafe { xpc_dictionary_get_bool(reply.0, OK_KEY.as_ptr().cast()) } {
            return Err("policy graph XPC service rejected its content-free handshake".into());
        }
        Ok(true)
    })();
    match begin_result {
        Ok(true) => Ok(connection),
        Ok(false) => Err("policy graph XPC service is busy with another projection".into()),
        Err(error) if connection.terminal_acknowledged.load(Ordering::Acquire) => {
            match connection.settle(false, deadline) {
                Ok(()) => Err(error),
                Err(settlement) => {
                    XPC_SETTLEMENT_FAILED.store(true, Ordering::Release);
                    Err(format!("{error}; {settlement}"))
                }
            }
        }
        Err(error) => {
            XPC_SETTLEMENT_FAILED.store(true, Ordering::Release);
            Err(format!(
                "{error}; policy graph XPC handshake had no terminal acknowledgement"
            ))
        }
    }
}

fn open_apple_speech_authenticated_connection(
    exact_cdhash: &[u8; 20],
    trusted_distribution: bool,
    deadline: Instant,
) -> Result<Connection, String> {
    let callback_queue = parent_callback_queue()?;
    let connection = unsafe {
        xpc_connection_create(
            APPLE_SPEECH_XPC_SERVICE_NAME.as_ptr().cast(),
            callback_queue,
        )
    };
    if connection.is_null() {
        return Err("Apple Speech XPC service could not be created".into());
    }
    let (invalidated_sender, invalidated) = mpsc::channel();
    let transport_failed = Arc::new(AtomicBool::new(false));
    let connection = Connection {
        subsystem: APPLE_SPEECH_SUBSYSTEM,
        object: connection,
        invalidated,
        service_nonce: Mutex::new(None),
        transport_failed: Arc::clone(&transport_failed),
        terminal_acknowledged: AtomicBool::new(false),
    };
    let encoded = exact_cdhash
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let mut requirement =
        format!("identifier \"com.useminutes.apple-speech-worker\" and cdhash H\"{encoded}\"");
    if trusted_distribution {
        requirement.push_str(
            " and anchor apple generic and certificate leaf[subject.OU] = \"63TMLKT8HN\"",
        );
    }
    let requirement = CString::new(requirement)
        .map_err(|_| "Apple Speech XPC requirement was malformed".to_string())?;
    set_peer_requirement(connection.object, &requirement)?;
    let events = RcBlock::new(move |event: XpcObject| {
        if xpc_is_connection_end(event) {
            transport_failed.store(true, Ordering::Release);
            let _ = invalidated_sender.send(());
        }
    });
    unsafe {
        xpc_connection_set_event_handler(connection.object, &events);
        xpc_connection_resume(connection.object);
    }

    // No utterance byte crosses the connection before this exact-service
    // code-signing requirement succeeds and the service returns its nonce.
    let begin_result = (|| {
        let begin = OwnedXpc::dictionary()?;
        set_command(begin.0, COMMAND_BEGIN);
        let reply = connection.send_with_reply(begin.0, deadline)?;
        if unsafe { xpc_dictionary_get_bool(reply.0, BUSY_KEY.as_ptr().cast()) } {
            return Ok(false);
        }
        if !unsafe { xpc_dictionary_get_bool(reply.0, OK_KEY.as_ptr().cast()) } {
            return Err("Apple Speech XPC service rejected its content-free handshake".into());
        }
        Ok(true)
    })();
    match begin_result {
        Ok(true) => Ok(connection),
        Ok(false) => Err("Apple Speech XPC service is busy with another utterance".into()),
        Err(error) if connection.terminal_acknowledged.load(Ordering::Acquire) => {
            match connection.settle(false, deadline) {
                Ok(()) => Err(error),
                Err(settlement) => {
                    APPLE_SPEECH_XPC_SETTLEMENT_FAILED.store(true, Ordering::Release);
                    Err(format!("{error}; {settlement}"))
                }
            }
        }
        Err(error) => {
            APPLE_SPEECH_XPC_SETTLEMENT_FAILED.store(true, Ordering::Release);
            Err(format!(
                "{error}; Apple Speech XPC handshake had no terminal acknowledgement"
            ))
        }
    }
}

pub(crate) fn run(
    authority_bundle: &Path,
    exact_cdhash: &[u8; 20],
    trusted_distribution: bool,
    mut input: impl Read,
    max_response_bytes: u64,
    wall_clock: Duration,
) -> Result<Vec<u8>, String> {
    if wall_clock.is_zero() {
        return Err("policy graph XPC wall-clock budget must be positive".into());
    }
    ensure_transport_available(GRAPH_SUBSYSTEM, &XPC_SETTLEMENT_FAILED)?;
    let deadline = Instant::now() + wall_clock;
    let _request_guard = lock_parent_request(
        GRAPH_SUBSYSTEM,
        &XPC_PARENT_REQUEST_LOCK,
        &XPC_SETTLEMENT_FAILED,
        deadline,
    )?;
    validate_authority_bundle(authority_bundle)?;
    let connection = open_authenticated_connection(exact_cdhash, trusted_distribution, deadline)?;

    let outcome = (|| {
        let mut sequence = 0_u64;
        let mut total_input = 0_u64;
        let mut buffer = [0_u8; XPC_CHUNK_BYTES];
        loop {
            let read = input
                .read(&mut buffer)
                .map_err(|_| "policy graph input stream could not be read".to_string())?;
            if read == 0 {
                break;
            }
            total_input = total_input
                .checked_add(read as u64)
                .filter(|total| *total <= crate::graph_worker::MAX_WORKER_INPUT_BYTES)
                .ok_or_else(|| "policy graph input exceeded its byte budget".to_string())?;
            let chunk = OwnedXpc::dictionary()?;
            set_command(chunk.0, COMMAND_CHUNK);
            unsafe {
                xpc_dictionary_set_uint64(chunk.0, SEQUENCE_KEY.as_ptr().cast(), sequence);
                xpc_dictionary_set_data(
                    chunk.0,
                    DATA_KEY.as_ptr().cast(),
                    buffer.as_ptr().cast(),
                    read,
                );
            }
            let reply = connection.send_with_reply(chunk.0, deadline)?;
            if !unsafe { xpc_dictionary_get_bool(reply.0, OK_KEY.as_ptr().cast()) } {
                return Err("policy graph XPC service rejected an input chunk".into());
            }
            sequence = sequence
                .checked_add(1)
                .ok_or_else(|| "policy graph XPC sequence overflowed".to_string())?;
        }

        let finish = OwnedXpc::dictionary()?;
        set_command(finish.0, COMMAND_FINISH);
        unsafe {
            xpc_dictionary_set_uint64(finish.0, SEQUENCE_KEY.as_ptr().cast(), sequence);
        }
        let reply = connection.send_with_reply(finish.0, deadline)?;
        if !unsafe { xpc_dictionary_get_bool(reply.0, OK_KEY.as_ptr().cast()) } {
            return Err("policy graph XPC worker failed closed".into());
        }
        let response_length =
            unsafe { xpc_dictionary_get_uint64(reply.0, LENGTH_KEY.as_ptr().cast()) };
        if response_length > max_response_bytes {
            return Err("policy graph XPC response exceeded its byte budget".into());
        }
        let capacity = usize::try_from(response_length)
            .map_err(|_| "policy graph XPC response exceeded this platform".to_string())?;
        let mut response = Vec::with_capacity(capacity);
        while response.len() < capacity {
            let pull = OwnedXpc::dictionary()?;
            set_command(pull.0, COMMAND_PULL);
            unsafe {
                xpc_dictionary_set_uint64(
                    pull.0,
                    OFFSET_KEY.as_ptr().cast(),
                    response.len() as u64,
                );
            }
            let reply = connection.send_with_reply(pull.0, deadline)?;
            if !unsafe { xpc_dictionary_get_bool(reply.0, OK_KEY.as_ptr().cast()) } {
                return Err("policy graph XPC service rejected a response pull".into());
            }
            let mut length = 0_usize;
            let data =
                unsafe { xpc_dictionary_get_data(reply.0, DATA_KEY.as_ptr().cast(), &mut length) };
            if data.is_null()
                || length == 0
                || length > XPC_CHUNK_BYTES
                || response
                    .len()
                    .checked_add(length)
                    .is_none_or(|end| end > capacity)
            {
                return Err("policy graph XPC service returned an invalid response chunk".into());
            }
            let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) };
            response.extend_from_slice(bytes);
        }
        Ok(response)
    })();
    let settlement = connection.settle(outcome.is_err(), deadline);
    match (outcome, settlement) {
        (Ok(response), Ok(())) => Ok(response),
        (Err(error), Ok(())) => Err(error),
        (outcome, Err(settlement)) => {
            XPC_SETTLEMENT_FAILED.store(true, Ordering::Release);
            let context = outcome
                .err()
                .unwrap_or_else(|| "policy graph XPC result was ready".to_string());
            Err(format!("{context}; {settlement}"))
        }
    }
}

pub(crate) fn run_apple_speech(
    authority_bundle: &Path,
    exact_cdhash: &[u8; 20],
    trusted_distribution: bool,
    mut input: impl Read,
    max_response_bytes: u64,
    wall_clock: Duration,
) -> Result<Vec<u8>, String> {
    if wall_clock.is_zero() {
        return Err("Apple Speech XPC wall-clock budget must be positive".into());
    }
    ensure_transport_available(APPLE_SPEECH_SUBSYSTEM, &APPLE_SPEECH_XPC_SETTLEMENT_FAILED)?;
    let deadline = Instant::now() + wall_clock;
    let _request_guard = lock_parent_request(
        APPLE_SPEECH_SUBSYSTEM,
        &APPLE_SPEECH_XPC_PARENT_REQUEST_LOCK,
        &APPLE_SPEECH_XPC_SETTLEMENT_FAILED,
        deadline,
    )?;
    validate_apple_speech_authority_bundle(authority_bundle)?;
    let connection =
        open_apple_speech_authenticated_connection(exact_cdhash, trusted_distribution, deadline)?;

    let outcome = (|| {
        let mut sequence = 0_u64;
        let mut total_input = 0_u64;
        let mut buffer = [0_u8; XPC_CHUNK_BYTES];
        loop {
            let read = input
                .read(&mut buffer)
                .map_err(|_| "Apple Speech input stream could not be read".to_string())?;
            if read == 0 {
                break;
            }
            total_input = total_input
                .checked_add(read as u64)
                .filter(|total| *total <= crate::apple_speech_worker::MAX_REQUEST_BYTES as u64)
                .ok_or_else(|| "Apple Speech input exceeded its byte budget".to_string())?;
            let chunk = OwnedXpc::dictionary()?;
            set_command(chunk.0, COMMAND_CHUNK);
            unsafe {
                xpc_dictionary_set_uint64(chunk.0, SEQUENCE_KEY.as_ptr().cast(), sequence);
                xpc_dictionary_set_data(
                    chunk.0,
                    DATA_KEY.as_ptr().cast(),
                    buffer.as_ptr().cast(),
                    read,
                );
            }
            let reply = connection.send_with_reply(chunk.0, deadline)?;
            if !unsafe { xpc_dictionary_get_bool(reply.0, OK_KEY.as_ptr().cast()) } {
                return Err("Apple Speech XPC service rejected an input chunk".into());
            }
            sequence = sequence
                .checked_add(1)
                .ok_or_else(|| "Apple Speech XPC sequence overflowed".to_string())?;
        }

        let finish = OwnedXpc::dictionary()?;
        set_command(finish.0, COMMAND_FINISH);
        unsafe {
            xpc_dictionary_set_uint64(finish.0, SEQUENCE_KEY.as_ptr().cast(), sequence);
        }
        let reply = connection.send_with_reply(finish.0, deadline)?;
        if !unsafe { xpc_dictionary_get_bool(reply.0, OK_KEY.as_ptr().cast()) } {
            return Err("Apple Speech XPC worker failed closed".into());
        }
        let response_length =
            unsafe { xpc_dictionary_get_uint64(reply.0, LENGTH_KEY.as_ptr().cast()) };
        if response_length == 0 || response_length > max_response_bytes {
            return Err("Apple Speech XPC response exceeded its byte budget".into());
        }
        let capacity = usize::try_from(response_length)
            .map_err(|_| "Apple Speech XPC response exceeded this platform".to_string())?;
        let mut response = Vec::with_capacity(capacity);
        while response.len() < capacity {
            let pull = OwnedXpc::dictionary()?;
            set_command(pull.0, COMMAND_PULL);
            unsafe {
                xpc_dictionary_set_uint64(
                    pull.0,
                    OFFSET_KEY.as_ptr().cast(),
                    response.len() as u64,
                );
            }
            let reply = connection.send_with_reply(pull.0, deadline)?;
            if !unsafe { xpc_dictionary_get_bool(reply.0, OK_KEY.as_ptr().cast()) } {
                return Err("Apple Speech XPC service rejected a response pull".into());
            }
            let mut length = 0_usize;
            let data =
                unsafe { xpc_dictionary_get_data(reply.0, DATA_KEY.as_ptr().cast(), &mut length) };
            if data.is_null()
                || length == 0
                || length > XPC_CHUNK_BYTES
                || response
                    .len()
                    .checked_add(length)
                    .is_none_or(|end| end > capacity)
            {
                return Err("Apple Speech XPC service returned an invalid response chunk".into());
            }
            let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) };
            response.extend_from_slice(bytes);
        }
        Ok(response)
    })();
    let settlement = connection.settle(outcome.is_err(), deadline);
    match (outcome, settlement) {
        (Ok(response), Ok(())) => Ok(response),
        (Err(error), Ok(())) => Err(error),
        (outcome, Err(settlement)) => {
            APPLE_SPEECH_XPC_SETTLEMENT_FAILED.store(true, Ordering::Release);
            let context = outcome
                .err()
                .unwrap_or_else(|| "Apple Speech XPC result was ready".to_string());
            Err(format!("{context}; {settlement}"))
        }
    }
}

enum ServicePhase {
    AwaitingBegin,
    Receiving {
        next_sequence: u64,
        input: Vec<u8>,
    },
    Processing,
    Responding {
        response: Vec<u8>,
        next_offset: usize,
    },
    Done,
}

impl ServicePhase {
    fn begin(&mut self) -> bool {
        if !matches!(self, Self::AwaitingBegin) {
            return false;
        }
        *self = Self::Receiving {
            next_sequence: 0,
            input: Vec::new(),
        };
        true
    }

    fn append_chunk(&mut self, sequence: u64, data: &[u8]) -> bool {
        let Self::Receiving {
            next_sequence,
            input,
        } = self
        else {
            return false;
        };
        if sequence != *next_sequence
            || data.is_empty()
            || data.len() > XPC_CHUNK_BYTES
            || input
                .len()
                .checked_add(data.len())
                .is_none_or(|total| total > crate::graph_worker::MAX_WORKER_INPUT_BYTES as usize)
        {
            return false;
        }
        input.extend_from_slice(data);
        let Some(next) = next_sequence.checked_add(1) else {
            return false;
        };
        *next_sequence = next;
        true
    }

    fn finish_input(&mut self, sequence: u64) -> Option<Vec<u8>> {
        let Self::Receiving {
            next_sequence,
            input,
        } = self
        else {
            return None;
        };
        if sequence != *next_sequence {
            return None;
        }
        let input = std::mem::take(input);
        *self = Self::Processing;
        Some(input)
    }

    fn install_response(&mut self, response: Vec<u8>) -> bool {
        if !matches!(self, Self::Processing) || response.is_empty() {
            return false;
        }
        *self = Self::Responding {
            response,
            next_offset: 0,
        };
        true
    }

    fn response_chunk(&mut self, offset: usize) -> Option<&[u8]> {
        let Self::Responding {
            response,
            next_offset,
        } = self
        else {
            return None;
        };
        if offset != *next_offset || offset >= response.len() {
            return None;
        }
        let end = offset.saturating_add(XPC_CHUNK_BYTES).min(response.len());
        *next_offset = end;
        Some(&response[offset..end])
    }

    fn response_complete(&self) -> bool {
        matches!(
            self,
            Self::Responding {
                response,
                next_offset
            } if *next_offset == response.len()
        )
    }

    fn abort(&mut self) -> bool {
        if matches!(self, Self::Done) {
            return false;
        }
        *self = Self::Done;
        true
    }
}

fn parent_requirement_for(
    verdict: TrustedDistribution,
    identifiers: &str,
) -> Result<String, String> {
    match verdict {
        TrustedDistribution::Yes => Ok(format!(
            "{identifiers} and anchor apple generic and certificate leaf[subject.OU] = \"63TMLKT8HN\""
        )),
        TrustedDistribution::No => Ok(identifiers.to_string()),
        TrustedDistribution::Indeterminate => Err(
            "XPC parent requirement could not establish this process's signing authority"
                .to_string(),
        ),
    }
}

fn service_parent_requirement() -> Result<CString, String> {
    let identifiers =
        "(identifier \"com.useminutes.desktop\" or identifier \"com.useminutes.desktop.dev\")";
    // A signed build must never accept the identifier-only form: any
    // ad-hoc-signed binary claiming the bundle id would satisfy it at the same
    // UID, which is exactly the adversary the hostile open-holder test models.
    // Downgrade only on a definitive "not a distribution build", never because
    // the evaluation could not complete.
    let requirement = parent_requirement_for(trusted_distribution_verdict(), identifiers)?;
    CString::new(requirement)
        .map_err(|_| "policy graph parent requirement was malformed".to_string())
}

fn service_reply(message: XpcObject, ok: bool) -> Option<OwnedXpc> {
    let reply = unsafe { xpc_dictionary_create_reply(message) };
    if reply.is_null() {
        return None;
    }
    unsafe { xpc_dictionary_set_bool(reply, OK_KEY.as_ptr().cast(), ok) };
    Some(OwnedXpc(reply))
}

fn service_command(message: XpcObject) -> Option<&'static str> {
    let command = unsafe { xpc_dictionary_get_string(message, COMMAND_KEY.as_ptr().cast()) };
    if command.is_null() {
        return None;
    }
    match unsafe { CStr::from_ptr(command) }.to_bytes() {
        b"begin" => Some("begin"),
        b"chunk" => Some("chunk"),
        b"finish" => Some("finish"),
        b"pull" => Some("pull"),
        b"abort" => Some("abort"),
        _ => None,
    }
}

fn handle_service_message(message: XpcObject, state: &Mutex<ServicePhase>) -> Option<OwnedXpc> {
    if !xpc_type_is(message, "dictionary") {
        return None;
    }
    let command = service_command(message)?;
    let mut phase = state.lock().ok()?;
    if command == "abort" {
        return service_reply(message, phase.abort());
    }
    match (&mut *phase, command) {
        (ServicePhase::AwaitingBegin, "begin") => {
            if !phase.begin() {
                return service_reply(message, false);
            }
            service_reply(message, true)
        }
        (ServicePhase::Receiving { .. }, "chunk") => {
            let sequence =
                unsafe { xpc_dictionary_get_uint64(message, SEQUENCE_KEY.as_ptr().cast()) };
            let mut length = 0_usize;
            let data =
                unsafe { xpc_dictionary_get_data(message, DATA_KEY.as_ptr().cast(), &mut length) };
            if data.is_null() {
                return service_reply(message, false);
            }
            let data = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) };
            service_reply(message, phase.append_chunk(sequence, data))
        }
        (ServicePhase::Receiving { .. }, "finish") => {
            let sequence =
                unsafe { xpc_dictionary_get_uint64(message, SEQUENCE_KEY.as_ptr().cast()) };
            let Some(input) = phase.finish_input(sequence) else {
                return service_reply(message, false);
            };
            let response = match crate::graph_worker::process_policy_projection_stream_bytes(&input)
            {
                Ok(response) => response,
                Err(_) => return service_reply(message, false),
            };
            let length = response.len() as u64;
            if !phase.install_response(response) {
                return service_reply(message, false);
            }
            let reply = service_reply(message, true)?;
            unsafe {
                xpc_dictionary_set_uint64(reply.0, LENGTH_KEY.as_ptr().cast(), length);
            }
            Some(reply)
        }
        (ServicePhase::Responding { .. }, "pull") => {
            let offset = usize::try_from(unsafe {
                xpc_dictionary_get_uint64(message, OFFSET_KEY.as_ptr().cast())
            })
            .ok()?;
            let reply = service_reply(message, true)?;
            let Some(chunk) = phase.response_chunk(offset) else {
                return service_reply(message, false);
            };
            unsafe {
                xpc_dictionary_set_data(
                    reply.0,
                    DATA_KEY.as_ptr().cast(),
                    chunk.as_ptr().cast(),
                    chunk.len(),
                );
            }
            let complete_after = phase.response_complete();
            if complete_after {
                *phase = ServicePhase::Done;
            }
            Some(reply)
        }
        _ => service_reply(message, false),
    }
}

enum AppleSpeechServicePhase {
    AwaitingBegin,
    Receiving {
        next_sequence: u64,
        input: zeroize::Zeroizing<Vec<u8>>,
    },
    Processing,
    Responding {
        response: Vec<u8>,
        next_offset: usize,
    },
    Done,
}

impl AppleSpeechServicePhase {
    fn begin(&mut self) -> bool {
        if !matches!(self, Self::AwaitingBegin) {
            return false;
        }
        *self = Self::Receiving {
            next_sequence: 0,
            input: zeroize::Zeroizing::new(Vec::new()),
        };
        true
    }

    fn append_chunk(&mut self, sequence: u64, data: &[u8]) -> bool {
        self.append_chunk_with_limit(
            sequence,
            data,
            crate::apple_speech_worker::MAX_REQUEST_BYTES,
        )
    }

    fn append_chunk_with_limit(
        &mut self,
        sequence: u64,
        data: &[u8],
        max_request_bytes: usize,
    ) -> bool {
        let Self::Receiving {
            next_sequence,
            input,
        } = self
        else {
            return false;
        };
        if sequence != *next_sequence
            || data.is_empty()
            || data.len() > XPC_CHUNK_BYTES
            || input
                .len()
                .checked_add(data.len())
                .is_none_or(|total| total > max_request_bytes)
        {
            return false;
        }
        input.extend_from_slice(data);
        let Some(next) = next_sequence.checked_add(1) else {
            return false;
        };
        *next_sequence = next;
        true
    }

    fn finish_input(&mut self, sequence: u64) -> Option<zeroize::Zeroizing<Vec<u8>>> {
        let Self::Receiving {
            next_sequence,
            input,
        } = self
        else {
            return None;
        };
        if sequence != *next_sequence {
            return None;
        }
        let input = std::mem::replace(input, zeroize::Zeroizing::new(Vec::new()));
        *self = Self::Processing;
        Some(input)
    }

    fn install_response(&mut self, response: Vec<u8>) -> bool {
        self.install_response_with_limit(
            response,
            crate::apple_speech_worker::MAX_RESPONSE_BYTES as usize,
        )
    }

    fn install_response_with_limit(
        &mut self,
        response: Vec<u8>,
        max_response_bytes: usize,
    ) -> bool {
        if !matches!(self, Self::Processing)
            || response.is_empty()
            || response.len() > max_response_bytes
        {
            return false;
        }
        *self = Self::Responding {
            response,
            next_offset: 0,
        };
        true
    }

    fn response_chunk(&mut self, offset: usize) -> Option<&[u8]> {
        let Self::Responding {
            response,
            next_offset,
        } = self
        else {
            return None;
        };
        if offset != *next_offset || offset >= response.len() {
            return None;
        }
        let end = offset.saturating_add(XPC_CHUNK_BYTES).min(response.len());
        *next_offset = end;
        Some(&response[offset..end])
    }

    fn response_complete(&self) -> bool {
        matches!(
            self,
            Self::Responding {
                response,
                next_offset
            } if *next_offset == response.len()
        )
    }

    fn abort(&mut self) -> bool {
        if matches!(self, Self::Done) {
            return false;
        }
        *self = Self::Done;
        true
    }
}

fn handle_apple_speech_service_message(
    message: XpcObject,
    state: &Mutex<AppleSpeechServicePhase>,
) -> Option<OwnedXpc> {
    if !xpc_type_is(message, "dictionary") {
        return None;
    }
    let command = service_command(message)?;
    let mut phase = state.lock().ok()?;
    if command == "abort" {
        return service_reply(message, phase.abort());
    }
    match (&mut *phase, command) {
        (AppleSpeechServicePhase::AwaitingBegin, "begin") => service_reply(message, phase.begin()),
        (AppleSpeechServicePhase::Receiving { .. }, "chunk") => {
            let sequence =
                unsafe { xpc_dictionary_get_uint64(message, SEQUENCE_KEY.as_ptr().cast()) };
            let mut length = 0_usize;
            let data =
                unsafe { xpc_dictionary_get_data(message, DATA_KEY.as_ptr().cast(), &mut length) };
            if data.is_null() {
                return service_reply(message, false);
            }
            let data = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) };
            service_reply(message, phase.append_chunk(sequence, data))
        }
        (AppleSpeechServicePhase::Receiving { .. }, "finish") => {
            let sequence =
                unsafe { xpc_dictionary_get_uint64(message, SEQUENCE_KEY.as_ptr().cast()) };
            let Some(input) = phase.finish_input(sequence) else {
                return service_reply(message, false);
            };
            let response = match crate::apple_speech_worker::process_private_audio_request(&input) {
                Ok(response) => response,
                Err(_) => return service_reply(message, false),
            };
            let length = response.len() as u64;
            if !phase.install_response(response) {
                return service_reply(message, false);
            }
            let reply = service_reply(message, true)?;
            unsafe {
                xpc_dictionary_set_uint64(reply.0, LENGTH_KEY.as_ptr().cast(), length);
            }
            Some(reply)
        }
        (AppleSpeechServicePhase::Responding { .. }, "pull") => {
            let offset = usize::try_from(unsafe {
                xpc_dictionary_get_uint64(message, OFFSET_KEY.as_ptr().cast())
            })
            .ok()?;
            let reply = service_reply(message, true)?;
            let Some(chunk) = phase.response_chunk(offset) else {
                return service_reply(message, false);
            };
            unsafe {
                xpc_dictionary_set_data(
                    reply.0,
                    DATA_KEY.as_ptr().cast(),
                    chunk.as_ptr().cast(),
                    chunk.len(),
                );
            }
            if phase.response_complete() {
                *phase = AppleSpeechServicePhase::Done;
            }
            Some(reply)
        }
        _ => service_reply(message, false),
    }
}

fn claim_service_process(claimed: &AtomicBool) -> bool {
    claimed
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

fn service_request_nonce_matches(message: XpcObject, expected: &[u8; 16]) -> bool {
    let mut length = 0_usize;
    let observed =
        unsafe { xpc_dictionary_get_data(message, SERVICE_NONCE_KEY.as_ptr().cast(), &mut length) };
    !observed.is_null()
        && length == expected.len()
        && unsafe { std::slice::from_raw_parts(observed.cast::<u8>(), length) } == expected
}

#[derive(Debug, PartialEq, Eq)]
enum ServicePeerEvent {
    HandleMessage,
    CancelPeer,
    ExitProcess,
}

fn classify_service_peer_event(
    is_dictionary: bool,
    owns_process_claim: bool,
    was_rejected: bool,
) -> ServicePeerEvent {
    if is_dictionary && !was_rejected {
        ServicePeerEvent::HandleMessage
    } else if owns_process_claim {
        ServicePeerEvent::ExitProcess
    } else {
        ServicePeerEvent::CancelPeer
    }
}

fn awaiting_command_can_claim(command: Option<&str>) -> bool {
    command == Some("begin")
}

fn new_service_process_nonce() -> Result<[u8; 16], String> {
    let mut nonce = [0_u8; 16];
    if unsafe { SecRandomCopyBytes(std::ptr::null(), nonce.len(), nonce.as_mut_ptr()) } == 0 {
        Ok(nonce)
    } else {
        Err("policy graph XPC service nonce generation failed".into())
    }
}

unsafe extern "C" fn graph_service_connection_handler(peer: XpcObject) {
    if !xpc_type_is(peer, "connection") {
        return;
    }
    let Ok(requirement) = service_parent_requirement() else {
        unsafe { xpc_connection_cancel(peer) };
        return;
    };
    if set_peer_requirement(peer, &requirement).is_err() {
        unsafe { xpc_connection_cancel(peer) };
        return;
    }
    let Some(message_service_nonce) = GRAPH_SERVICE_NONCE.get().copied() else {
        unsafe { xpc_connection_cancel(peer) };
        return;
    };
    let state = Arc::new(Mutex::new(ServicePhase::AwaitingBegin));
    let message_state = Arc::clone(&state);
    let message_claimed = &GRAPH_SERVICE_CLAIMED;
    let peer_owns_process_claim = Arc::new(AtomicBool::new(false));
    let message_peer_owns_process_claim = Arc::clone(&peer_owns_process_claim);
    let peer_was_rejected = Arc::new(AtomicBool::new(false));
    let message_peer_was_rejected = Arc::clone(&peer_was_rejected);
    let peer_address = peer as usize;
    let messages = RcBlock::new(move |message: XpcObject| {
        match classify_service_peer_event(
            xpc_type_is(message, "dictionary"),
            message_peer_owns_process_claim.load(Ordering::Acquire),
            message_peer_was_rejected.load(Ordering::Acquire),
        ) {
            ServicePeerEvent::HandleMessage => {}
            ServicePeerEvent::CancelPeer => {
                unsafe { xpc_connection_cancel(peer_address as XpcObject) };
                return;
            }
            ServicePeerEvent::ExitProcess => unsafe { libc::_exit(72) },
        }
        let command = service_command(message);
        let awaiting_begin = message_state
            .lock()
            .is_ok_and(|phase| matches!(*phase, ServicePhase::AwaitingBegin));
        if awaiting_begin && !awaiting_command_can_claim(command) {
            let Some(reply) = service_reply(message, false) else {
                unsafe { xpc_connection_cancel(peer_address as XpcObject) };
                return;
            };
            unsafe {
                xpc_dictionary_set_bool(reply.0, TERMINAL_KEY.as_ptr().cast(), false);
                xpc_dictionary_set_data(
                    reply.0,
                    SERVICE_NONCE_KEY.as_ptr().cast(),
                    message_service_nonce.as_ptr().cast(),
                    message_service_nonce.len(),
                );
                xpc_connection_send_message(peer_address as XpcObject, reply.0);
            }
            return;
        }
        if awaiting_begin && !claim_service_process(message_claimed) {
            message_peer_was_rejected.store(true, Ordering::Release);
            let Some(reply) = service_reply(message, false) else {
                unsafe { xpc_connection_cancel(peer_address as XpcObject) };
                return;
            };
            unsafe {
                xpc_dictionary_set_bool(reply.0, BUSY_KEY.as_ptr().cast(), true);
                xpc_dictionary_set_bool(reply.0, TERMINAL_KEY.as_ptr().cast(), false);
                xpc_dictionary_set_data(
                    reply.0,
                    SERVICE_NONCE_KEY.as_ptr().cast(),
                    message_service_nonce.as_ptr().cast(),
                    message_service_nonce.len(),
                );
                xpc_connection_send_message(peer_address as XpcObject, reply.0);
            }
            return;
        }
        if awaiting_begin {
            message_peer_owns_process_claim.store(true, Ordering::Release);
        } else if !service_request_nonce_matches(message, &message_service_nonce) {
            let Some(reply) = service_reply(message, false) else {
                unsafe { libc::_exit(71) };
            };
            unsafe {
                xpc_dictionary_set_bool(reply.0, TERMINAL_KEY.as_ptr().cast(), true);
                xpc_dictionary_set_data(
                    reply.0,
                    SERVICE_NONCE_KEY.as_ptr().cast(),
                    message_service_nonce.as_ptr().cast(),
                    message_service_nonce.len(),
                );
                xpc_connection_send_message(peer_address as XpcObject, reply.0);
            }
            let exit_after_send = RcBlock::new(|| unsafe { libc::_exit(71) });
            unsafe {
                xpc_connection_send_barrier(peer_address as XpcObject, &exit_after_send);
            }
            return;
        }
        let Some(reply) = handle_service_message(message, &message_state) else {
            unsafe { libc::_exit(71) };
        };
        let ok = unsafe { xpc_dictionary_get_bool(reply.0, OK_KEY.as_ptr().cast()) };
        let terminal = !ok
            || message_state
                .lock()
                .is_ok_and(|phase| matches!(*phase, ServicePhase::Done));
        unsafe {
            xpc_dictionary_set_bool(reply.0, TERMINAL_KEY.as_ptr().cast(), terminal);
            xpc_dictionary_set_data(
                reply.0,
                SERVICE_NONCE_KEY.as_ptr().cast(),
                message_service_nonce.as_ptr().cast(),
                message_service_nonce.len(),
            );
            xpc_connection_send_message(peer_address as XpcObject, reply.0);
        }
        if terminal {
            let exit_after_send = RcBlock::new(|| unsafe { libc::_exit(0) });
            unsafe {
                xpc_connection_send_barrier(peer_address as XpcObject, &exit_after_send);
            }
        }
    });
    unsafe {
        xpc_connection_set_event_handler(peer, &messages);
        xpc_connection_resume(peer);
    }
}

pub fn run_service_main() -> ! {
    // XPC may otherwise reuse one helper process for sequential connections.
    // This service is intentionally one authenticated request per process:
    // immutable hard limits and one non-extendable wall timer are installed
    // before the first connection, and the process exits after its terminal
    // reply is sent.
    if crate::graph_worker::prepare_macos_graph_xpc_worker().is_err() {
        unsafe { libc::_exit(70) };
    }
    let service_nonce = match new_service_process_nonce() {
        Ok(nonce) => nonce,
        Err(_) => unsafe { libc::_exit(70) },
    };
    if GRAPH_SERVICE_NONCE.set(service_nonce).is_err() {
        unsafe { libc::_exit(70) };
    }
    unsafe { xpc_main(graph_service_connection_handler) }
}

unsafe extern "C" fn apple_speech_service_connection_handler(peer: XpcObject) {
    if !xpc_type_is(peer, "connection") {
        return;
    }
    let Ok(requirement) = service_parent_requirement() else {
        unsafe { xpc_connection_cancel(peer) };
        return;
    };
    if set_peer_requirement(peer, &requirement).is_err() {
        unsafe { xpc_connection_cancel(peer) };
        return;
    }
    let Some(message_service_nonce) = APPLE_SPEECH_SERVICE_NONCE.get().copied() else {
        unsafe { xpc_connection_cancel(peer) };
        return;
    };
    let state = Arc::new(Mutex::new(AppleSpeechServicePhase::AwaitingBegin));
    let message_state = Arc::clone(&state);
    let message_claimed = &APPLE_SPEECH_SERVICE_CLAIMED;
    let peer_owns_process_claim = Arc::new(AtomicBool::new(false));
    let message_peer_owns_process_claim = Arc::clone(&peer_owns_process_claim);
    let peer_was_rejected = Arc::new(AtomicBool::new(false));
    let message_peer_was_rejected = Arc::clone(&peer_was_rejected);
    let peer_address = peer as usize;
    let messages = RcBlock::new(move |message: XpcObject| {
        match classify_service_peer_event(
            xpc_type_is(message, "dictionary"),
            message_peer_owns_process_claim.load(Ordering::Acquire),
            message_peer_was_rejected.load(Ordering::Acquire),
        ) {
            ServicePeerEvent::HandleMessage => {}
            ServicePeerEvent::CancelPeer => {
                unsafe { xpc_connection_cancel(peer_address as XpcObject) };
                return;
            }
            ServicePeerEvent::ExitProcess => unsafe { libc::_exit(72) },
        }
        let command = service_command(message);
        let awaiting_begin = message_state
            .lock()
            .is_ok_and(|phase| matches!(*phase, AppleSpeechServicePhase::AwaitingBegin));
        if awaiting_begin && !awaiting_command_can_claim(command) {
            let Some(reply) = service_reply(message, false) else {
                unsafe { xpc_connection_cancel(peer_address as XpcObject) };
                return;
            };
            unsafe {
                xpc_dictionary_set_bool(reply.0, TERMINAL_KEY.as_ptr().cast(), false);
                xpc_dictionary_set_data(
                    reply.0,
                    SERVICE_NONCE_KEY.as_ptr().cast(),
                    message_service_nonce.as_ptr().cast(),
                    message_service_nonce.len(),
                );
                xpc_connection_send_message(peer_address as XpcObject, reply.0);
            }
            return;
        }
        if awaiting_begin && !claim_service_process(message_claimed) {
            message_peer_was_rejected.store(true, Ordering::Release);
            let Some(reply) = service_reply(message, false) else {
                unsafe { xpc_connection_cancel(peer_address as XpcObject) };
                return;
            };
            unsafe {
                xpc_dictionary_set_bool(reply.0, BUSY_KEY.as_ptr().cast(), true);
                xpc_dictionary_set_bool(reply.0, TERMINAL_KEY.as_ptr().cast(), false);
                xpc_dictionary_set_data(
                    reply.0,
                    SERVICE_NONCE_KEY.as_ptr().cast(),
                    message_service_nonce.as_ptr().cast(),
                    message_service_nonce.len(),
                );
                xpc_connection_send_message(peer_address as XpcObject, reply.0);
            }
            return;
        }
        if awaiting_begin {
            message_peer_owns_process_claim.store(true, Ordering::Release);
        } else if !service_request_nonce_matches(message, &message_service_nonce) {
            let Some(reply) = service_reply(message, false) else {
                unsafe { libc::_exit(71) };
            };
            unsafe {
                xpc_dictionary_set_bool(reply.0, TERMINAL_KEY.as_ptr().cast(), true);
                xpc_dictionary_set_data(
                    reply.0,
                    SERVICE_NONCE_KEY.as_ptr().cast(),
                    message_service_nonce.as_ptr().cast(),
                    message_service_nonce.len(),
                );
                xpc_connection_send_message(peer_address as XpcObject, reply.0);
            }
            let exit_after_send = RcBlock::new(|| unsafe { libc::_exit(71) });
            unsafe {
                xpc_connection_send_barrier(peer_address as XpcObject, &exit_after_send);
            }
            return;
        }
        let Some(reply) = handle_apple_speech_service_message(message, &message_state) else {
            unsafe { libc::_exit(71) };
        };
        let ok = unsafe { xpc_dictionary_get_bool(reply.0, OK_KEY.as_ptr().cast()) };
        let terminal = !ok
            || message_state
                .lock()
                .is_ok_and(|phase| matches!(*phase, AppleSpeechServicePhase::Done));
        unsafe {
            xpc_dictionary_set_bool(reply.0, TERMINAL_KEY.as_ptr().cast(), terminal);
            xpc_dictionary_set_data(
                reply.0,
                SERVICE_NONCE_KEY.as_ptr().cast(),
                message_service_nonce.as_ptr().cast(),
                message_service_nonce.len(),
            );
            xpc_connection_send_message(peer_address as XpcObject, reply.0);
        }
        if terminal {
            let exit_after_send = RcBlock::new(|| unsafe { libc::_exit(0) });
            unsafe {
                xpc_connection_send_barrier(peer_address as XpcObject, &exit_after_send);
            }
        }
    });
    unsafe {
        xpc_connection_set_event_handler(peer, &messages);
        xpc_connection_resume(peer);
    }
}

pub fn run_apple_speech_service_main() -> ! {
    // One authenticated utterance per process. Immutable resource ceilings
    // are installed before XPC accepts a peer and cannot be raised by a later
    // request.
    if crate::apple_speech_worker::prepare_macos_apple_speech_xpc_worker().is_err() {
        unsafe { libc::_exit(70) };
    }
    let service_nonce = match new_service_process_nonce() {
        Ok(nonce) => nonce,
        Err(_) => unsafe { libc::_exit(70) },
    };
    if APPLE_SPEECH_SERVICE_NONCE.set(service_nonce).is_err() {
        unsafe { libc::_exit(70) };
    }
    unsafe { xpc_main(apple_speech_service_connection_handler) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_protocol_rejects_replay_reordering_and_premature_transitions() {
        let mut phase = ServicePhase::AwaitingBegin;
        assert!(phase.response_chunk(0).is_none());
        assert!(phase.finish_input(0).is_none());
        assert!(phase.begin());
        assert!(!phase.begin());
        assert!(!phase.append_chunk(1, b"late"));
        assert!(!phase.append_chunk(0, b""));
        assert!(!phase.append_chunk(0, &vec![0; XPC_CHUNK_BYTES + 1]));
        assert!(phase.append_chunk(0, b"first"));
        assert!(!phase.append_chunk(0, b"replay"));
        assert!(!phase.append_chunk(2, b"skip"));
        assert!(phase.append_chunk(1, b"second"));
        assert!(phase.finish_input(1).is_none());
        assert_eq!(phase.finish_input(2).unwrap(), b"firstsecond");
        assert!(phase.install_response(b"response".to_vec()));
        assert!(!phase.install_response(b"replacement".to_vec()));
        assert!(phase.response_chunk(1).is_none());
        assert_eq!(phase.response_chunk(0).unwrap(), b"response");
        assert!(phase.response_complete());
    }

    #[test]
    fn service_abort_is_terminal_from_every_live_phase() {
        let mut awaiting = ServicePhase::AwaitingBegin;
        assert!(awaiting.abort());
        assert!(!awaiting.abort());

        let mut receiving = ServicePhase::AwaitingBegin;
        assert!(receiving.begin());
        assert!(receiving.abort());
        assert!(!receiving.abort());

        let mut responding = ServicePhase::Processing;
        assert!(responding.install_response(b"result".to_vec()));
        assert!(responding.abort());
        assert!(!responding.abort());
    }

    #[test]
    fn apple_speech_protocol_rejects_replay_reordering_and_premature_transitions() {
        let mut phase = AppleSpeechServicePhase::AwaitingBegin;
        assert!(phase.response_chunk(0).is_none());
        assert!(phase.finish_input(0).is_none());
        assert!(!phase.install_response(b"early".to_vec()));
        assert!(phase.begin());
        assert!(!phase.begin());
        assert!(!phase.append_chunk(1, b"late"));
        assert!(!phase.append_chunk(0, b""));
        assert!(!phase.append_chunk(0, &vec![0; XPC_CHUNK_BYTES + 1]));
        assert!(phase.append_chunk(0, b"first"));
        assert!(!phase.append_chunk(0, b"replay"));
        assert!(!phase.append_chunk(2, b"skip"));
        assert!(phase.append_chunk(1, b"second"));
        assert!(phase.finish_input(1).is_none());
        assert_eq!(phase.finish_input(2).unwrap().as_slice(), b"firstsecond");
        assert!(phase.install_response(b"response".to_vec()));
        assert!(!phase.install_response(b"replacement".to_vec()));
        assert!(phase.response_chunk(1).is_none());
        assert_eq!(phase.response_chunk(0).unwrap(), b"response");
        assert!(phase.response_complete());
        assert!(phase.response_chunk(0).is_none());
    }

    #[test]
    fn apple_speech_protocol_enforces_aggregate_request_and_response_budgets() {
        let mut request = AppleSpeechServicePhase::AwaitingBegin;
        assert!(request.begin());
        assert!(request.append_chunk_with_limit(0, b"1234", 4));
        assert!(!request.append_chunk_with_limit(1, b"5", 4));
        assert_eq!(request.finish_input(1).unwrap().as_slice(), b"1234");

        assert!(!request.install_response_with_limit(Vec::new(), 4));
        assert!(!request.install_response_with_limit(b"12345".to_vec(), 4));
        assert!(request.install_response_with_limit(b"1234".to_vec(), 4));
        assert!(request.response_chunk(1).is_none());
        assert_eq!(request.response_chunk(0).unwrap(), b"1234");
        assert!(request.response_complete());
    }

    #[test]
    fn apple_speech_abort_is_terminal_from_every_live_phase() {
        let mut awaiting = AppleSpeechServicePhase::AwaitingBegin;
        assert!(awaiting.abort());
        assert!(!awaiting.abort());

        let mut receiving = AppleSpeechServicePhase::AwaitingBegin;
        assert!(receiving.begin());
        assert!(receiving.abort());
        assert!(!receiving.abort());

        let mut processing = AppleSpeechServicePhase::Processing;
        assert!(processing.abort());
        assert!(!processing.abort());

        let mut responding = AppleSpeechServicePhase::Processing;
        assert!(responding.install_response(b"result".to_vec()));
        assert!(responding.abort());
        assert!(!responding.abort());
    }

    #[test]
    fn apple_speech_one_process_claim_and_disconnect_are_fail_closed() {
        let claimed = AtomicBool::new(false);
        assert!(claim_service_process(&claimed));
        assert!(!claim_service_process(&claimed));
        assert_eq!(
            classify_service_peer_event(false, false, true),
            ServicePeerEvent::CancelPeer
        );
        assert_eq!(
            classify_service_peer_event(false, true, false),
            ServicePeerEvent::ExitProcess
        );
    }

    #[test]
    fn signed_builds_never_fall_open_to_an_identifier_only_peer_requirement() {
        let identifiers = "(identifier \"com.useminutes.desktop\")";
        // A definitive trusted-distribution verdict anchors to the team.
        let anchored = parent_requirement_for(TrustedDistribution::Yes, identifiers).unwrap();
        assert!(anchored.contains("anchor apple generic"));
        assert!(anchored.contains("63TMLKT8HN"));
        // A definitive "not a distribution build" keeps local development working.
        assert_eq!(
            parent_requirement_for(TrustedDistribution::No, identifiers).unwrap(),
            identifiers
        );
        // An evaluation that could not complete must fail closed rather than
        // install a requirement any ad-hoc binary at the same UID satisfies.
        assert!(parent_requirement_for(TrustedDistribution::Indeterminate, identifiers).is_err());
    }

    #[test]
    fn only_an_exact_success_or_requirement_failure_is_a_verdict() {
        assert_eq!(verdict_from_status(1), TrustedDistribution::Yes);
        assert_eq!(verdict_from_status(0), TrustedDistribution::No);
        for status in [-1, -67050, 2, i32::MIN, i32::MAX] {
            assert_eq!(
                verdict_from_status(status),
                TrustedDistribution::Indeterminate,
                "status {status} must not be read as a verdict"
            );
        }
    }

    #[test]
    fn settlement_binds_every_request_to_one_service_process_nonce() {
        let first = [1_u8; 16];
        let second = [2_u8; 16];
        let mut expected = None;
        bind_service_nonce(GRAPH_SUBSYSTEM, &mut expected, first).unwrap();
        bind_service_nonce(GRAPH_SUBSYSTEM, &mut expected, first).unwrap();
        assert!(bind_service_nonce(GRAPH_SUBSYSTEM, &mut expected, second).is_err());
        assert_eq!(expected, Some(first));
    }

    #[test]
    fn service_admission_rejects_stale_abort_without_claiming_fresh_process() {
        assert!(awaiting_command_can_claim(Some("begin")));
        assert!(!awaiting_command_can_claim(Some("abort")));
        assert!(!awaiting_command_can_claim(Some("chunk")));
        assert!(!awaiting_command_can_claim(None));
    }

    #[test]
    fn busy_peer_disconnect_does_not_retire_process_claim_owner() {
        assert_eq!(
            classify_service_peer_event(false, false, true),
            ServicePeerEvent::CancelPeer
        );
        assert_eq!(
            classify_service_peer_event(false, false, false),
            ServicePeerEvent::CancelPeer
        );
        assert_eq!(
            classify_service_peer_event(false, true, false),
            ServicePeerEvent::ExitProcess
        );
        assert_eq!(
            classify_service_peer_event(true, true, false),
            ServicePeerEvent::HandleMessage
        );
    }

    #[test]
    fn request_waiter_rechecks_transport_poison_after_lock_admission() {
        let lock = Arc::new(Mutex::new(()));
        let poisoned = Arc::new(AtomicBool::new(false));
        let owner = lock.lock().unwrap();
        let waiter_lock = Arc::clone(&lock);
        let waiter_poisoned = Arc::clone(&poisoned);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let waiter = std::thread::spawn(move || {
            ready_tx.send(()).unwrap();
            lock_parent_request(
                GRAPH_SUBSYSTEM,
                &waiter_lock,
                &waiter_poisoned,
                Instant::now() + Duration::from_secs(1),
            )
            .is_err()
        });
        ready_rx.recv().unwrap();
        poisoned.store(true, Ordering::Release);
        drop(owner);
        assert!(waiter.join().unwrap());
    }

    #[test]
    fn service_process_nonces_are_fresh_and_nonzero() {
        let first = new_service_process_nonce().unwrap();
        let second = new_service_process_nonce().unwrap();
        assert_ne!(first, [0_u8; 16]);
        assert_ne!(second, [0_u8; 16]);
        assert_ne!(first, second);
    }

    #[test]
    fn service_protocol_chunks_one_response_in_exact_offset_order() {
        let mut phase = ServicePhase::AwaitingBegin;
        assert!(phase.begin());
        assert!(phase.append_chunk(0, b"request"));
        assert_eq!(phase.finish_input(1).unwrap(), b"request");
        let response = vec![7; XPC_CHUNK_BYTES + 3];
        assert!(phase.install_response(response));
        assert_eq!(phase.response_chunk(0).unwrap().len(), XPC_CHUNK_BYTES);
        assert!(phase.response_chunk(0).is_none());
        assert_eq!(phase.response_chunk(XPC_CHUNK_BYTES).unwrap().len(), 3);
        assert!(phase.response_complete());
    }

    #[test]
    fn service_process_claim_is_one_shot() {
        let claimed = AtomicBool::new(false);
        assert!(claim_service_process(&claimed));
        assert!(!claim_service_process(&claimed));
    }
}
