//! Host API functions provided to WASM guest modules.

use super::guest_abi::GuestExports;
use crate::modules::manifest::{is_plain_file_name, ModuleManifest};
use crate::modules::ModuleMessage;
use anyhow::Result;
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::mpsc;
use wasmtime::{Caller, Linker, StoreLimits};
use zbus::zvariant::{DynamicTuple, DynamicType, OwnedObjectPath, Signature, Type, Value};

/// Maximum allowed socket payload in bytes (1 MiB).
pub const MAX_SOCKET_PAYLOAD_BYTES: usize = 1024 * 1024;
pub const MAX_READ_FILE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_EXEC_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_EXEC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const MAX_SOCKET_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

fn connect_unix_socket_with_timeout(
    path: &std::path::Path,
) -> std::io::Result<std::os::unix::net::UnixStream> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::{FromRawFd, OwnedFd};
    let path_bytes = path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if path_bytes.is_empty()
        || path_bytes.contains(&0)
        || path_bytes.len() >= address.sun_path.len()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Unix socket path is empty or too long",
        ));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    // SAFETY: path_bytes fits in sun_path with a trailing NUL byte, and both buffers are valid.
    unsafe {
        std::ptr::copy_nonoverlapping(
            path_bytes.as_ptr().cast::<libc::c_char>(),
            address.sun_path.as_mut_ptr(),
            path_bytes.len(),
        );
    }
    let address_len = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + path_bytes.len() + 1)
        as libc::socklen_t;
    // SAFETY: socket arguments are valid for a nonblocking Unix-domain stream socket.
    let raw_fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if raw_fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: raw_fd is a newly-created file descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    // SAFETY: address and address_len describe the initialized sockaddr_un above.
    let result = unsafe {
        libc::connect(
            raw_fd,
            (&address as *const libc::sockaddr_un).cast::<libc::sockaddr>(),
            address_len,
        )
    };
    if result < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(error);
        }

        let mut poll_fd = libc::pollfd {
            fd: raw_fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: poll_fd points to one initialized descriptor record.
        let ready = unsafe {
            libc::poll(
                &mut poll_fd,
                1,
                MAX_SOCKET_CONNECT_TIMEOUT.as_millis() as libc::c_int,
            )
        };
        if ready == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Unix socket connection timed out",
            ));
        }
        if ready < 0 {
            return Err(std::io::Error::last_os_error());
        }

        let mut socket_error: libc::c_int = 0;
        let mut socket_error_len = std::mem::size_of_val(&socket_error) as libc::socklen_t;
        // SAFETY: both output pointers reference valid initialized storage.
        let result = unsafe {
            libc::getsockopt(
                raw_fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut socket_error as *mut libc::c_int).cast(),
                &mut socket_error_len,
            )
        };
        if result < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if socket_error != 0 {
            return Err(std::io::Error::from_raw_os_error(socket_error));
        }
    }

    // SAFETY: fd is exclusively owned here and now contains a connected Unix stream.
    let stream = std::os::unix::net::UnixStream::from(fd);
    stream.set_nonblocking(false)?;
    Ok(stream)
}

fn effective_read_path(path: &str) -> std::path::PathBuf {
    let path_obj = std::path::Path::new(path);
    if path_obj.is_absolute() {
        return path_obj.to_path_buf();
    }
    let runtime_path = crate::state_files::get_runtime_dir().join(path_obj);
    if runtime_path.exists() {
        runtime_path
    } else {
        let temp_path = std::env::temp_dir().join("wyrd").join(path_obj);
        if temp_path.exists() {
            temp_path
        } else {
            runtime_path
        }
    }
}

fn resolve_unix_socket_path(
    path: &std::path::Path,
    runtime_dir: Option<std::path::PathBuf>,
) -> std::result::Result<std::path::PathBuf, i32> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    let runtime_dir = runtime_dir.ok_or(-1)?;
    if !runtime_dir.is_absolute() {
        return Err(-1);
    }
    Ok(runtime_dir.join(path))
}

fn run_command_with_timeout(
    program: &std::path::Path,
    args: &[String],
) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    use std::time::Instant;

    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = command.spawn()?;
    let Some(mut stdout) = child.stdout.take() else {
        return Err(std::io::Error::other("failed to capture child stdout"));
    };
    let output = Arc::new(Mutex::new(Vec::new()));
    let reader_output = output.clone();
    let (reader_done_tx, reader_done_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let mut captured = lock_unpoisoned(&reader_output);
                    let remaining = MAX_EXEC_OUTPUT_BYTES.saturating_sub(captured.len());
                    captured.extend_from_slice(&chunk[..read.min(remaining)]);
                }
            }
        }
        let _ = reader_done_tx.send(());
    });

    let deadline = Instant::now() + MAX_EXEC_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            #[cfg(unix)]
            {
                // SAFETY: the negative child process ID targets the process group created for this command.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "child process exceeded execution timeout",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let _ = reader_done_rx.recv_timeout(std::time::Duration::from_millis(100));
    let stdout = std::mem::take(&mut *lock_unpoisoned(&output));
    Ok(std::process::Output {
        status,
        stdout,
        stderr: Vec::new(),
    })
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Close reason for a subscribed Unix socket.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CloseReason {
    PeerClosed,
    ReadError(String),
    Shutdown,
}

/// Event received from a subscribed socket.
///
/// Frames are newline-delimited; the newline is excluded from the raw-byte payload.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SocketEvent {
    Data { handle: u32, payload: Vec<u8> },
    Closed { handle: u32, reason: CloseReason },
    Error { handle: u32, code: i32 },
}

static NEXT_SUBSCRIPTION_HANDLE: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(1);

type ActiveSocketEntry = (
    std::sync::Arc<std::sync::atomic::AtomicBool>,
    Option<std::os::unix::net::UnixStream>,
);
static ACTIVE_SUBSCRIPTIONS: LazyLock<Mutex<HashMap<u32, ActiveSocketEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static NEXT_TIMER_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
type ActiveTimerEntry = (std::sync::Arc<std::sync::atomic::AtomicBool>, String);
static ACTIVE_TIMERS: LazyLock<Mutex<HashMap<u32, ActiveTimerEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn unregister_timer(timer_id: u32) {
    lock_unpoisoned(&ACTIVE_TIMERS).remove(&timer_id);
}

pub fn active_timers_count() -> usize {
    lock_unpoisoned(&ACTIVE_TIMERS).len()
}

pub fn active_timers_for_module(name: &str) -> usize {
    lock_unpoisoned(&ACTIVE_TIMERS)
        .values()
        .filter(|(_, mod_name)| mod_name == name)
        .count()
}

pub fn is_timer_active(timer_id: u32) -> bool {
    lock_unpoisoned(&ACTIVE_TIMERS).contains_key(&timer_id)
}

pub struct PathWatchEntry {
    pub _watcher: Arc<Mutex<Option<notify::RecommendedWatcher>>>,
    pub module_name: String,
    pub target_path: std::path::PathBuf,
    pub is_cancelled: Arc<std::sync::atomic::AtomicBool>,
}

static NEXT_PATH_WATCH_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
static ACTIVE_PATH_WATCHES: LazyLock<Mutex<HashMap<u32, PathWatchEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub static ACTIVE_ESCALATION_THREADS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

pub fn active_escalations_count() -> usize {
    ACTIVE_ESCALATION_THREADS.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn unregister_path_watch(watch_id: u32) -> bool {
    if let Some(entry) = lock_unpoisoned(&ACTIVE_PATH_WATCHES).remove(&watch_id) {
        entry
            .is_cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = lock_unpoisoned(&entry._watcher).take();
        true
    } else {
        false
    }
}

pub fn active_watches_count() -> usize {
    lock_unpoisoned(&ACTIVE_PATH_WATCHES).len()
}

pub fn active_watches_for_module(name: &str) -> usize {
    lock_unpoisoned(&ACTIVE_PATH_WATCHES)
        .values()
        .filter(|e| e.module_name == name)
        .count()
}

pub fn is_watch_active(watch_id: u32) -> bool {
    lock_unpoisoned(&ACTIVE_PATH_WATCHES).contains_key(&watch_id)
}

fn shutdown_socket(stream: &std::os::unix::net::UnixStream, handle: u32) {
    if let Err(error) = stream.shutdown(std::net::Shutdown::Both) {
        if error.kind() != std::io::ErrorKind::NotConnected {
            warn!(
                "Failed to shut down Unix socket subscription {}: {}",
                handle, error
            );
        }
    }
}

pub fn close_subscription(handle: u32) -> i32 {
    if let Some((is_shutdown, stream_opt)) = lock_unpoisoned(&ACTIVE_SUBSCRIPTIONS).remove(&handle)
    {
        is_shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(stream) = stream_opt {
            shutdown_socket(&stream, handle);
        }
        0
    } else {
        -1
    }
}

pub fn run_socket_reader(
    handle: u32,
    stream: std::os::unix::net::UnixStream,
    event_tx: mpsc::Sender<SocketEvent>,
    is_shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::io::BufRead;
    let mut reader = std::io::BufReader::new(stream);
    let mut buf = Vec::new();

    loop {
        if is_shutdown.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = event_tx.blocking_send(SocketEvent::Closed {
                handle,
                reason: CloseReason::Shutdown,
            });
            break;
        }
        buf.clear();
        let mut line_complete = false;
        let mut read_error = None;

        while buf.len() <= MAX_SOCKET_PAYLOAD_BYTES {
            let available = match reader.fill_buf() {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    read_error = Some(e.to_string());
                    break;
                }
            };

            if available.is_empty() {
                // EOF
                break;
            }

            if let Some(pos) = available.iter().position(|&b| b == b'\n') {
                if buf.len() + pos > MAX_SOCKET_PAYLOAD_BYTES {
                    read_error = Some("payload too large".to_string());
                    break;
                }
                buf.extend_from_slice(&available[..pos]);
                reader.consume(pos + 1);
                line_complete = true;
                break;
            } else {
                if buf.len() + available.len() > MAX_SOCKET_PAYLOAD_BYTES {
                    read_error = Some("payload too large".to_string());
                    break;
                }
                let chunk_len = available.len();
                buf.extend_from_slice(available);
                reader.consume(chunk_len);
            }
        }

        if is_shutdown.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = event_tx.blocking_send(SocketEvent::Closed {
                handle,
                reason: CloseReason::Shutdown,
            });
            break;
        }

        if let Some(err_msg) = read_error {
            warn!("socket {handle}: {err_msg}, closing");
            let _ = event_tx.blocking_send(SocketEvent::Error { handle, code: -1 });
            let _ = event_tx.blocking_send(SocketEvent::Closed {
                handle,
                reason: CloseReason::ReadError(err_msg),
            });
            break;
        }

        if !line_complete {
            // EOF reached
            let _ = event_tx.blocking_send(SocketEvent::Closed {
                handle,
                reason: CloseReason::PeerClosed,
            });
            break;
        }

        if buf.ends_with(b"\r") {
            buf.pop();
        }

        if event_tx
            .blocking_send(SocketEvent::Data {
                handle,
                payload: buf.clone(),
            })
            .is_err()
        {
            break;
        }
    }
}

pub struct ModuleHostState {
    pub name: String,
    pub manifest: ModuleManifest,
    pub updates_tx: mpsc::Sender<(String, ModuleMessage)>,
    pub dbus_signal_tx: mpsc::Sender<(String, String, String, String)>,
    pub socket_event_tx: mpsc::Sender<SocketEvent>,
    pub timer_event_tx: mpsc::Sender<u32>,
    pub path_event_tx: mpsc::Sender<u32>,
    pub subscribed_signals: std::collections::HashSet<(String, String, String, String, String)>,
    pub last_updates: std::collections::HashMap<String, serde_json::Value>,
    pub active_timers: std::collections::HashSet<u32>,
    pub active_watches: std::collections::HashSet<u32>,
    pub active_sockets: HashMap<u32, std::os::unix::net::UnixStream>,
    pub limits: StoreLimits,
}

impl ModuleHostState {
    pub fn cleanup_timers(&mut self) {
        let mut timers = lock_unpoisoned(&ACTIVE_TIMERS);
        for timer_id in self.active_timers.drain() {
            if let Some((is_cancelled, _)) = timers.remove(&timer_id) {
                is_cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }

    pub fn cleanup_watches(&mut self) {
        let mut watches = lock_unpoisoned(&ACTIVE_PATH_WATCHES);
        for watch_id in self.active_watches.drain() {
            if let Some(entry) = watches.remove(&watch_id) {
                entry
                    .is_cancelled
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                let _ = lock_unpoisoned(&entry._watcher).take();
            }
        }
    }

    pub fn cleanup_sockets(&mut self) {
        for (handle, stream) in self.active_sockets.drain() {
            close_subscription(handle);
            shutdown_socket(&stream, handle);
        }
    }
}

impl Drop for ModuleHostState {
    fn drop(&mut self) {
        self.cleanup_timers();
        self.cleanup_watches();
        self.cleanup_sockets();
    }
}

fn dispatch_child_reaper(rx: std::sync::mpsc::Receiver<std::process::Child>) {
    std::thread::spawn(move || {
        while let Ok(mut child) = rx.recv() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    });
}

static CHILD_REAPER: LazyLock<std::sync::mpsc::Sender<std::process::Child>> = LazyLock::new(|| {
    let (tx, rx) = std::sync::mpsc::channel::<std::process::Child>();
    dispatch_child_reaper(rx);
    tx
});

fn resolve_binary_in_path(prog_name: &str) -> Option<std::path::PathBuf> {
    if !is_plain_file_name(prog_name) {
        return None;
    }
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(prog_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn is_systemd_user_available() -> bool {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(|dir| std::path::Path::new(&dir).join("systemd/private").exists())
        .unwrap_or(false)
        && resolve_binary_in_path("systemd-run").is_some()
}

fn run_blocking<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread {
            return tokio::task::block_in_place(f);
        }
    }
    f()
}

pub fn register_host_functions(linker: &mut Linker<ModuleHostState>) -> Result<()> {
    linker.func_wrap(
        "wyrd_host",
        "host_send_update",
        |mut caller: Caller<'_, ModuleHostState>,
         widget_ptr: u32,
         widget_len: u32,
         payload_ptr: u32,
         payload_len: u32|
         -> u32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return 1,
            };

            let widget_id =
                match GuestExports::read_string(&memory, &caller, widget_ptr, widget_len) {
                    Ok(s) => s,
                    Err(e) => {
                        error!("host_send_update failed reading widget_id: {}", e);
                        return 2;
                    }
                };

            let payload_str =
                match GuestExports::read_string(&memory, &caller, payload_ptr, payload_len) {
                    Ok(s) => s,
                    Err(e) => {
                        error!("host_send_update failed reading payload: {}", e);
                        return 3;
                    }
                };

            let payload: serde_json::Value = match serde_json::from_str(&payload_str) {
                Ok(v) => v,
                Err(e) => {
                    error!("host_send_update invalid JSON payload: {}", e);
                    return 4;
                }
            };

            if caller.data().last_updates.get(&widget_id) == Some(&payload) {
                return 0;
            }
            caller
                .data_mut()
                .last_updates
                .insert(widget_id.clone(), payload.clone());

            let module_name = caller.data().name.clone();
            let updates_tx = caller.data().updates_tx.clone();

            if let Err(e) =
                updates_tx.try_send((module_name, ModuleMessage::Update { widget_id, payload }))
            {
                warn!("host_send_update failed to send update: {}", e);
                return 5;
            }
            0
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_request_surface",
        |mut caller: Caller<'_, ModuleHostState>,
         action_ptr: u32,
         action_len: u32,
         params_ptr: u32,
         params_len: u32|
         -> u32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return 1,
            };

            let action = match GuestExports::read_string(&memory, &caller, action_ptr, action_len) {
                Ok(s) => s,
                Err(e) => {
                    error!("host_request_surface failed reading action: {}", e);
                    return 2;
                }
            };

            let params_str =
                match GuestExports::read_string(&memory, &caller, params_ptr, params_len) {
                    Ok(s) => s,
                    Err(e) => {
                        error!("host_request_surface failed reading params: {}", e);
                        return 3;
                    }
                };

            let params: serde_json::Value = match serde_json::from_str(&params_str) {
                Ok(v) => v,
                Err(e) => {
                    error!("host_request_surface invalid JSON params: {}", e);
                    return 4;
                }
            };

            let module_name = caller.data().name.clone();
            let updates_tx = caller.data().updates_tx.clone();

            if let Err(e) =
                updates_tx.try_send((module_name, ModuleMessage::Surface { action, params }))
            {
                warn!("host_request_surface failed to send surface request: {}", e);
                return 5;
            }
            0
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_publish",
        |mut caller: Caller<'_, ModuleHostState>,
         topic_ptr: u32,
         topic_len: u32,
         val_ptr: u32,
         val_len: u32|
         -> u32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return 1,
            };

            let topic = match GuestExports::read_string(&memory, &caller, topic_ptr, topic_len) {
                Ok(s) => s,
                Err(e) => {
                    error!("host_publish failed reading topic: {}", e);
                    return 2;
                }
            };

            // Capability enforcement
            if !caller.data().manifest.can_publish(&topic) {
                warn!(
                    "Module '{}' attempted to publish to unauthorized topic '{}'",
                    caller.data().name,
                    topic
                );
                return 403; // Permission Denied
            }

            let val_str = match GuestExports::read_string(&memory, &caller, val_ptr, val_len) {
                Ok(s) => s,
                Err(e) => {
                    error!("host_publish failed reading value: {}", e);
                    return 3;
                }
            };

            let value: serde_json::Value = match serde_json::from_str(&val_str) {
                Ok(v) => v,
                Err(e) => {
                    error!("host_publish invalid JSON value: {}", e);
                    return 4;
                }
            };

            let module_name = caller.data().name.clone();
            let updates_tx = caller.data().updates_tx.clone();

            if let Err(e) =
                updates_tx.try_send((module_name, ModuleMessage::Publish { topic, value }))
            {
                warn!("host_publish failed to send publish message: {}", e);
                return 5;
            }
            0
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_request_capability",
        |mut caller: Caller<'_, ModuleHostState>,
         cap_ptr: u32,
         cap_len: u32,
         action_ptr: u32,
         action_len: u32,
         params_ptr: u32,
         params_len: u32|
         -> u32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return 1,
            };

            let capability = match GuestExports::read_string(&memory, &caller, cap_ptr, cap_len) {
                Ok(s) => s,
                Err(e) => {
                    error!("host_request_capability failed reading capability: {}", e);
                    return 2;
                }
            };

            // Capability broker enforcement
            if !caller.data().manifest.has_capability(&capability) {
                warn!(
                    "Module '{}' requested unauthorized capability '{}'",
                    caller.data().name,
                    capability
                );
                return 403; // Permission Denied
            }

            let action = match GuestExports::read_string(&memory, &caller, action_ptr, action_len) {
                Ok(s) => s,
                Err(e) => {
                    error!("host_request_capability failed reading action: {}", e);
                    return 3;
                }
            };

            let params_str =
                match GuestExports::read_string(&memory, &caller, params_ptr, params_len) {
                    Ok(s) => s,
                    Err(e) => {
                        error!("host_request_capability failed reading params: {}", e);
                        return 4;
                    }
                };

            let params: serde_json::Value = match serde_json::from_str(&params_str) {
                Ok(v) => v,
                Err(e) => {
                    error!("host_request_capability invalid JSON params: {}", e);
                    return 5;
                }
            };

            let module_name = caller.data().name.clone();
            let updates_tx = caller.data().updates_tx.clone();

            if let Err(e) = updates_tx.try_send((
                module_name,
                ModuleMessage::RequestCapability {
                    capability,
                    action,
                    params,
                },
            )) {
                warn!(
                    "host_request_capability failed to send capability request: {}",
                    e
                );
                return 6;
            }
            0
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_focus_request",
        |mut caller: Caller<'_, ModuleHostState>, popup_ptr: u32, popup_len: u32| -> u32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return 1,
            };

            if !caller.data().manifest.can_request_focus() {
                warn!(
                    "Module '{}' denied focus request (missing focus:request capability)",
                    caller.data().name
                );
                return 1;
            }

            let popup_id = match GuestExports::read_string(&memory, &caller, popup_ptr, popup_len) {
                Ok(s) => s,
                Err(e) => {
                    error!("host_focus_request failed reading popup_id: {}", e);
                    return 2;
                }
            };

            let module_name = caller.data().name.clone();
            let updates_tx = caller.data().updates_tx.clone();

            if let Err(e) =
                updates_tx.try_send((module_name, ModuleMessage::FocusRequest { popup_id }))
            {
                warn!("host_focus_request failed to send focus request: {}", e);
                return 3;
            }
            0
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_log",
        |mut caller: Caller<'_, ModuleHostState>, level: u32, msg_ptr: u32, msg_len: u32| {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return,
            };

            if let Ok(msg) = GuestExports::read_string(&memory, &caller, msg_ptr, msg_len) {
                let name = &caller.data().name;
                match level {
                    0 => log::trace!("[WASM:{}] {}", name, msg),
                    1 => log::debug!("[WASM:{}] {}", name, msg),
                    2 => info!("[WASM:{}] {}", name, msg),
                    3 => warn!("[WASM:{}] {}", name, msg),
                    _ => error!("[WASM:{}] {}", name, msg),
                }
            }
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_now_ms",
        |_caller: Caller<'_, ModuleHostState>| -> u64 {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_local_offset_seconds",
        |_caller: Caller<'_, ModuleHostState>| -> i32 {
            // SAFETY: localtime_r initializes `tm` when it returns a non-null pointer.
            unsafe {
                let now = libc::time(std::ptr::null_mut());
                let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
                if libc::localtime_r(&now, tm.as_mut_ptr()).is_null() {
                    return 0;
                }
                let tm = tm.assume_init();
                tm.tm_gmtoff as i32
            }
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_read_file",
        |mut caller: Caller<'_, ModuleHostState>,
         path_ptr: u32,
         path_len: u32,
         out_ptr: u32,
         out_max_len: u32|
         -> i32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };

            let path_str = match GuestExports::read_string(&memory, &caller, path_ptr, path_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };

            let effective_path = effective_read_path(&path_str);

            if !caller.data().manifest.can_read_path(&path_str)
                && !caller
                    .data()
                    .manifest
                    .can_read_path(&effective_path.to_string_lossy())
            {
                warn!(
                    "Module '{}' denied reading file '{}' (missing fs:read capability)",
                    caller.data().name,
                    path_str
                );
                return -403;
            }

            let metadata = match std::fs::symlink_metadata(&effective_path) {
                Ok(metadata) if metadata.file_type().is_file() => metadata,
                Ok(_) => return -403,
                Err(_) => return -4,
            };
            if metadata.len() > MAX_READ_FILE_BYTES as u64 {
                return -6;
            }
            let mut open_options = std::fs::OpenOptions::new();
            open_options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                open_options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            }
            let file = match open_options.open(&effective_path) {
                Ok(file) => file,
                Err(_) => return -4,
            };
            if !file
                .metadata()
                .is_ok_and(|metadata| metadata.file_type().is_file())
            {
                return -403;
            }
            let mut bytes =
                Vec::with_capacity(metadata.len().min(MAX_READ_FILE_BYTES as u64) as usize);
            let read_result = file
                .take((MAX_READ_FILE_BYTES + 1) as u64)
                .read_to_end(&mut bytes);
            if read_result.is_err() {
                return -4;
            }
            if bytes.len() > MAX_READ_FILE_BYTES || bytes.len() > out_max_len as usize {
                return -6;
            }
            let Ok(content) = std::str::from_utf8(&bytes) else {
                return -4;
            };
            let mem_data = memory.data_mut(&mut caller);
            let start = out_ptr as usize;
            let Some(end) = start.checked_add(bytes.len()) else {
                return -3;
            };
            if end <= mem_data.len() {
                mem_data[start..end].copy_from_slice(content.as_bytes());
                bytes.len() as i32
            } else {
                -3
            }
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_write_file",
        |_caller: Caller<'_, ModuleHostState>,
         _path_ptr: u32,
         _path_len: u32,
         _content_ptr: u32,
         _content_len: u32|
         -> i32 {
            warn!("host_write_file is unsupported in wyrd-wasm 0.1.0");
            -403
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_read_dir",
        |mut caller: Caller<'_, ModuleHostState>,
         path_ptr: u32,
         path_len: u32,
         out_ptr: u32,
         out_max_len: u32|
         -> i32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };

            let path_str = match GuestExports::read_string(&memory, &caller, path_ptr, path_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };

            let effective_path = effective_read_path(&path_str);
            if !caller.data().manifest.can_read_path(&path_str)
                && !caller
                    .data()
                    .manifest
                    .can_read_path(&effective_path.to_string_lossy())
            {
                warn!(
                    "Module '{}' denied reading directory '{}' (capability missing)",
                    caller.data().name,
                    path_str
                );
                return -403;
            }

            let rd = match std::fs::read_dir(&effective_path) {
                Ok(rd) => rd,
                Err(_) => return -1,
            };
            let mut names = Vec::new();
            for entry in rd {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => return -1,
                };
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                names.push(name);
                if names.len() > 4096 {
                    return -6;
                }
            }
            let entries = match serde_json::to_string(&names) {
                Ok(entries) => entries,
                Err(_) => return -1,
            };
            let bytes = entries.as_bytes();
            if bytes.len() > out_max_len as usize || bytes.len() > MAX_READ_FILE_BYTES {
                return -6;
            }
            let len = bytes.len();
            let mem_data = memory.data_mut(&mut caller);
            let start = out_ptr as usize;
            let Some(end) = start.checked_add(len) else {
                return -3;
            };
            if end <= mem_data.len() {
                mem_data[start..end].copy_from_slice(bytes);
                len as i32
            } else {
                -3
            }
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_exec_process",
        |mut caller: Caller<'_, ModuleHostState>,
         prog_ptr: u32,
         prog_len: u32,
         args_ptr: u32,
         args_len: u32,
         out_ptr: u32,
         out_max_len: u32|
         -> i32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };

            let prog = match GuestExports::read_string(&memory, &caller, prog_ptr, prog_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };

            let args_json = match GuestExports::read_string(&memory, &caller, args_ptr, args_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };

            let args: Vec<String> = match serde_json::from_str(&args_json) {
                Ok(a) => a,
                Err(e) => {
                    error!("host_exec_process invalid args JSON for prog '{}': {}", prog, e);
                    return -2;
                }
            };

            let prog_name = std::path::Path::new(&prog)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&prog);

            if !is_plain_file_name(&prog)
                || !caller.data().manifest.can_spawn_binary(&prog)
            {
                warn!(
                    "Module '{}' denied executing path-bearing program {:?}",
                    caller.data().name,
                    prog
                );
                return -403;
            }
            if !caller.data().manifest.can_spawn_binary(prog_name) {
                warn!(
                    "Module '{}' denied executing program '{}' (missing process:spawn:{} capability)",
                    caller.data().name,
                    prog,
                    prog_name
                );
                return -403;
            }

            let resolved_path = match resolve_binary_in_path(&prog).or_else(|| resolve_binary_in_path(prog_name)) {
                Some(p) => p,
                None => {
                    warn!("host_exec_process binary '{}' not found in $PATH", prog);
                    return -4;
                }
            };

            if out_max_len == 0 {
                let systemd_available = is_systemd_user_available();
                let mut spawned_child = None;

                if systemd_available {
                    let mut cmd = std::process::Command::new("systemd-run");
                    cmd.arg("--user")
                        .arg("--scope")
                        .arg("--slice=app.slice")
                        .arg("-q")
                        .arg("--")
                        .arg(&resolved_path)
                        .args(&args);
                    cmd.stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null());
                    #[cfg(unix)]
                    {
                        use std::os::unix::process::CommandExt;
                        // SAFETY: setsid, signal, and prctl are async-signal-safe POSIX calls.
                        unsafe {
                            cmd.pre_exec(|| {
                                let _ = libc::syscall(
                                    libc::SYS_close_range,
                                    3usize,
                                    usize::MAX,
                                    libc::CLOSE_RANGE_CLOEXEC as usize,
                                );
                                let _ = libc::setsid();
                                libc::signal(libc::SIGHUP, libc::SIG_IGN);
                                let _ = libc::prctl(libc::PR_SET_PDEATHSIG, 0, 0, 0, 0);
                                Ok(())
                            });
                        }
                    }
                    if let Ok(child) = cmd.spawn() {
                        spawned_child = Some(child);
                    } else {
                        warn!(
                            "host_exec_process systemd-run launch failed for '{}', falling back to direct spawn",
                            prog_name
                        );
                    }
                }

                if spawned_child.is_none() {
                    let mut cmd = std::process::Command::new(&resolved_path);
                    cmd.args(&args);
                    cmd.stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null());
                    #[cfg(unix)]
                    {
                        use std::os::unix::process::CommandExt;
                        // SAFETY: setsid, signal, and prctl are async-signal-safe POSIX calls.
                        unsafe {
                            cmd.pre_exec(|| {
                                let _ = libc::syscall(
                                    libc::SYS_close_range,
                                    3usize,
                                    usize::MAX,
                                    libc::CLOSE_RANGE_CLOEXEC as usize,
                                );
                                let _ = libc::setsid();
                                libc::signal(libc::SIGHUP, libc::SIG_IGN);
                                let _ = libc::prctl(libc::PR_SET_PDEATHSIG, 0, 0, 0, 0);
                                Ok(())
                            });
                        }
                    }
                    match cmd.spawn() {
                        Ok(child) => spawned_child = Some(child),
                        Err(e) => {
                            warn!("host_exec_process failed to spawn '{}': {}", prog_name, e);
                            return -4;
                        }
                    }
                }

                return match spawned_child {
                    Some(child) => {
                        let _ = CHILD_REAPER.send(child);
                        0
                    }
                    None => -4,
                };
            }

            let output = run_blocking(|| run_command_with_timeout(&resolved_path, &args));

            match output {
                Ok(out) => {
                    if !out.status.success() {
                        warn!(
                            "host_exec_process '{}' exited with non-zero status: {:?}",
                            prog_name,
                            out.status.code()
                        );
                        return -5;
                    }
                    let bytes = out.stdout;
                    let len = bytes.len().min(out_max_len as usize);
                    let mem_data = memory.data_mut(&mut caller);
                    let start = out_ptr as usize;
                    if start + len <= mem_data.len() {
                        mem_data[start..start + len].copy_from_slice(&bytes[..len]);
                        len as i32
                    } else {
                        -3
                    }
                }
                Err(e) => {
                    warn!("host_exec_process failed to execute '{}': {}", prog_name, e);
                    if e.kind() == std::io::ErrorKind::TimedOut {
                        -7
                    } else {
                        -4
                    }
                }
            }
        },
    )?;

    // Tokenize cmd_str and execute the binary directly without invoking a shell.
    linker.func_wrap(
        "wyrd_host",
        "host_exec_command",
        |mut caller: Caller<'_, ModuleHostState>,
         cmd_ptr: u32,
         cmd_len: u32,
         out_ptr: u32,
         out_max_len: u32|
         -> i32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };

            let cmd_str = match GuestExports::read_string(&memory, &caller, cmd_ptr, cmd_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };

            let tokens = match shell_words::split(&cmd_str) {
                Ok(t) if !t.is_empty() => t,
                _ => return -2,
            };

            let prog = &tokens[0];
            let args = &tokens[1..];

            let prog_name = std::path::Path::new(prog)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(prog);

            if !is_plain_file_name(prog) || !caller.data().manifest.can_spawn_binary(prog) {
                warn!(
                    "Module '{}' denied executing path-bearing program {:?}",
                    caller.data().name,
                    prog
                );
                return -403;
            }
            if !caller.data().manifest.can_spawn_binary(prog_name) {
                warn!(
                    "Module '{}' denied executing command '{}' (missing process:spawn:{} capability)",
                    caller.data().name,
                    cmd_str,
                    prog_name
                );
                return -403;
            }

            let resolved_path = match resolve_binary_in_path(prog).or_else(|| resolve_binary_in_path(prog_name)) {
                Some(p) => p,
                None => {
                    warn!("host_exec_command binary '{}' not found in $PATH", prog);
                    return -4;
                }
            };

            let output = run_blocking(|| run_command_with_timeout(&resolved_path, args));

            match output {
                Ok(out) => {
                    if !out.status.success() {
                        warn!(
                            "host_exec_command '{}' exited with non-zero status: {:?}",
                            prog_name,
                            out.status.code()
                        );
                        // -5: Command exited with non-zero exit status (distinct from spawn failure -4)
                        return -5;
                    }
                    let bytes = out.stdout;
                    let len = bytes.len().min(out_max_len as usize);
                    let mem_data = memory.data_mut(&mut caller);
                    let start = out_ptr as usize;
                    if start + len <= mem_data.len() {
                        mem_data[start..start + len].copy_from_slice(&bytes[..len]);
                        len as i32
                    } else {
                        -3
                    }
                }
                Err(e) => {
                    warn!("host_exec_command failed to execute '{}': {}", prog_name, e);
                    if e.kind() == std::io::ErrorKind::TimedOut {
                        -7
                    } else {
                        -4
                    }
                }
            }
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_dbus_call",
        |mut caller: Caller<'_, ModuleHostState>,
         bus_ptr: u32,
         bus_len: u32,
         service_ptr: u32,
         service_len: u32,
         path_ptr: u32,
         path_len: u32,
         iface_ptr: u32,
         iface_len: u32,
         member_ptr: u32,
         member_len: u32,
         args_ptr: u32,
         args_len: u32,
         out_ptr: u32,
         out_max_len: u32|
         -> i32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };

            let bus = match GuestExports::read_string(&memory, &caller, bus_ptr, bus_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };
            let service = match GuestExports::read_string(&memory, &caller, service_ptr, service_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };
            let path = match GuestExports::read_string(&memory, &caller, path_ptr, path_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };
            let iface = match GuestExports::read_string(&memory, &caller, iface_ptr, iface_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };
            let member = match GuestExports::read_string(&memory, &caller, member_ptr, member_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };
            let args_str = match GuestExports::read_string(&memory, &caller, args_ptr, args_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };

            if !caller.data().manifest.can_dbus_call(&bus, &service) {
                warn!(
                    "Module '{}' denied calling D-Bus service '{}:{}' (missing capability dbus:call:{}:{})",
                    caller.data().name,
                    bus,
                    service,
                    bus,
                    service
                );
                return -403;
            }

            let result = run_blocking(|| {
                perform_dbus_call(&bus, &service, &path, &iface, &member, &args_str)
            });

            match result {
                Ok(json_str) => {
                    let bytes = json_str.as_bytes();
                    let len = bytes.len().min(out_max_len as usize);
                    let mem_data = memory.data_mut(&mut caller);
                    let start = out_ptr as usize;
                    if start + len <= mem_data.len() {
                        mem_data[start..start + len].copy_from_slice(&bytes[..len]);
                        len as i32
                    } else {
                        -3
                    }
                }
                Err(e) => {
                    debug!("D-Bus call '{}:{} {}::{}' failed: {}", service, path, iface, member, e);
                    -500
                }
            }
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_dbus_subscribe",
        |mut caller: Caller<'_, ModuleHostState>,
         bus_ptr: u32,
         bus_len: u32,
         service_ptr: u32,
         service_len: u32,
         path_ptr: u32,
         path_len: u32,
         iface_ptr: u32,
         iface_len: u32,
         member_ptr: u32,
         member_len: u32|
         -> i32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };

            let bus = match GuestExports::read_string(&memory, &caller, bus_ptr, bus_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };
            let service = match GuestExports::read_string(&memory, &caller, service_ptr, service_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };
            let path = match GuestExports::read_string(&memory, &caller, path_ptr, path_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };
            let iface = match GuestExports::read_string(&memory, &caller, iface_ptr, iface_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };
            let member = match GuestExports::read_string(&memory, &caller, member_ptr, member_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };

            if !caller.data().manifest.can_dbus_subscribe(&bus, &service, &iface) {
                warn!(
                    "Module '{}' denied subscribing to D-Bus signal '{}:{}:{}' (missing capability dbus:subscribe:{}:{}:{})",
                    caller.data().name,
                    bus,
                    service,
                    iface,
                    bus,
                    service,
                    iface
                );
                return -403;
            }

            let sub_key = (
                bus.clone(),
                service.clone(),
                path.clone(),
                iface.clone(),
                member.clone(),
            );
            if !caller.data_mut().subscribed_signals.insert(sub_key) {
                return 0;
            }

            let signal_tx = caller.data().dbus_signal_tx.clone();
            tokio::spawn(async move {
                subscribe_dbus_signal(bus, service, path, iface, member, signal_tx).await;
            });

            0
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_file_exists",
        |mut caller: Caller<'_, ModuleHostState>, path_ptr: u32, path_len: u32| -> i32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };

            let path_str = match GuestExports::read_string(&memory, &caller, path_ptr, path_len) {
                Ok(s) => s,
                Err(_) => return -1,
            };

            if !caller.data().manifest.can_read_path(&path_str) {
                warn!(
                    "Module '{}' denied checking file existence '{}' (missing fs:read capability)",
                    caller.data().name,
                    path_str
                );
                return -1;
            }

            let local_path = path_str.strip_prefix("file://").unwrap_or(&path_str);
            if std::path::Path::new(local_path).exists() {
                1
            } else {
                0
            }
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_image_cached",
        |mut caller: Caller<'_, ModuleHostState>, path_ptr: u32, path_len: u32| -> i32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };

            let path_str = match GuestExports::read_string(&memory, &caller, path_ptr, path_len) {
                Ok(s) => s,
                Err(_) => return -1,
            };

            let is_remote_url = path_str
                .get(..7)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("http://"))
                || path_str
                    .get(..8)
                    .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"));
            if is_remote_url {
                if !caller.data().manifest.can_network_fetch(&path_str) {
                    warn!(
                        "Module '{}' denied fetching image URL '{}' (missing network:fetch capability)",
                        caller.data().name,
                        path_str
                    );
                    return -403;
                }
            } else if !caller.data().manifest.can_read_path(&path_str) {
                warn!(
                    "Module '{}' denied reading cached image '{}' (missing fs:read capability)",
                    caller.data().name,
                    path_str
                );
                return -403;
            }

            let resolved = if is_remote_url {
                crate::render::resolve_or_cache_image_path_with_policy(
                    &path_str,
                    crate::render::default_image_fetcher().as_ref(),
                    true,
                )
            } else {
                crate::render::resolve_or_cache_image_path(&path_str)
            };
            if resolved.is_some() {
                1
            } else {
                0
            }
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_list_dir",
        |mut caller: Caller<'_, ModuleHostState>,
         path_ptr: u32,
         path_len: u32,
         out_ptr: u32,
         out_max_len: u32|
         -> i32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };

            let path_str = match GuestExports::read_string(&memory, &caller, path_ptr, path_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };

            if !caller.data().manifest.can_read_path(&path_str) {
                warn!(
                    "Module '{}' denied reading directory '{}' (capability missing)",
                    caller.data().name,
                    path_str
                );
                return -403;
            }

            let mut files = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&path_str) {
                for entry in entries.flatten() {
                    if let Ok(file_type) = entry.file_type() {
                        if file_type.is_file() || file_type.is_symlink() {
                            files.push(entry.path().to_string_lossy().to_string());
                        }
                    }
                }
            }

            let json_str = serde_json::to_string(&files).unwrap_or_else(|_| "[]".to_string());
            let bytes = json_str.as_bytes();
            let len = bytes.len().min(out_max_len as usize);
            let mem_data = memory.data_mut(&mut caller);
            let start = out_ptr as usize;
            if start + len <= mem_data.len() {
                mem_data[start..start + len].copy_from_slice(&bytes[..len]);
                len as i32
            } else {
                -3
            }
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_unix_socket_request",
        |mut caller: Caller<'_, ModuleHostState>,
         path_ptr: u32,
         path_len: u32,
         payload_ptr: u32,
         payload_len: u32,
         out_ptr: u32,
         out_max_len: u32|
         -> i32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };

            let path_str = match GuestExports::read_string(&memory, &caller, path_ptr, path_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };
            let path_obj = std::path::Path::new(&path_str);
            if path_str.chars().any(char::is_control)
                || (path_obj.is_relative() && !is_plain_file_name(&path_str))
            {
                warn!(
                    "Module '{}' requested invalid Unix socket path {:?}",
                    caller.data().name,
                    path_str
                );
                return -400;
            }

            let payload_str =
                match GuestExports::read_string(&memory, &caller, payload_ptr, payload_len) {
                    Ok(s) => s,
                    Err(_) => return -2,
                };
            if payload_str.len() > MAX_SOCKET_PAYLOAD_BYTES {
                return -6;
            }

            let effective_path = match resolve_unix_socket_path(
                path_obj,
                std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from),
            ) {
                Ok(path) => path,
                Err(_) => {
                    warn!(
                        "Module '{}' denied relative Unix socket '{}': XDG_RUNTIME_DIR is unset or not absolute",
                        caller.data().name,
                        path_str
                    );
                    return -1;
                }
            };

            if !caller.data().manifest.can_unix_socket_connect(&path_str)
                && !caller
                    .data()
                    .manifest
                    .can_unix_socket_connect(&effective_path.to_string_lossy())
            {
                warn!(
                    "Module '{}' denied connecting to Unix socket '{}' (capability missing)",
                    caller.data().name,
                    path_str
                );
                return -403;
            }

            let socket_res = run_blocking(|| -> Result<String, i32> {
                use std::io::{BufRead, BufReader, Write};
                use std::time::Duration;

                let mut stream =
                    connect_unix_socket_with_timeout(&effective_path).map_err(|e| {
                        debug!(
                            "host_unix_socket_request connect to {:?} failed: {}",
                            effective_path, e
                        );
                        if e.kind() == std::io::ErrorKind::TimedOut {
                            -7
                        } else {
                            -404
                        }
                    })?;

                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

                stream.write_all(payload_str.as_bytes()).map_err(|e| {
                    warn!("host_unix_socket_request write failed: {}", e);
                    -4
                })?;
                if !payload_str.ends_with('\n') {
                    stream.write_all(b"\n").map_err(|_| -4)?;
                }
                stream.flush().map_err(|_| -4)?;

                let mut reader = BufReader::new(stream).take((MAX_SOCKET_PAYLOAD_BYTES + 1) as u64);
                let mut response = Vec::new();
                reader.read_until(b'\n', &mut response).map_err(|e| {
                    warn!("host_unix_socket_request read failed: {}", e);
                    -4
                })?;
                if response.len() > MAX_SOCKET_PAYLOAD_BYTES {
                    return Err(-6);
                }

                Ok(String::from_utf8_lossy(&response).into_owned())
            });

            match socket_res {
                Ok(response) => {
                    let bytes = response.trim_end_matches(['\r', '\n']).as_bytes();
                    let len = bytes.len().min(out_max_len as usize);
                    let mem_data = memory.data_mut(&mut caller);
                    let start = out_ptr as usize;
                    if start + len <= mem_data.len() {
                        mem_data[start..start + len].copy_from_slice(&bytes[..len]);
                        len as i32
                    } else {
                        -3
                    }
                }
                Err(code) => code,
            }
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_unix_socket_subscribe",
        |mut caller: Caller<'_, ModuleHostState>, socket_ptr: u32, socket_len: u32| -> i32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(memory) => memory,
                None => return -1,
            };
            let socket_name =
                match GuestExports::read_string(&memory, &caller, socket_ptr, socket_len) {
                    Ok(value) => value,
                    Err(_) => return -2,
                };
            if !is_plain_file_name(&socket_name) {
                warn!(
                    "Module '{}' requested invalid Unix socket name {:?}",
                    caller.data().name,
                    socket_name
                );
                return -400;
            }
            if !caller.data().manifest.can_unix_socket_connect(&socket_name) {
                warn!(
                    "Module '{}' denied connecting to Unix socket '{}' (capability missing)",
                    caller.data().name,
                    socket_name
                );
                return -403;
            }

            let path = match resolve_unix_socket_path(
                std::path::Path::new(&socket_name),
                std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from),
            ) {
                Ok(path) => path,
                Err(_) => {
                    warn!(
                        "Module '{}' denied relative Unix socket '{}': XDG_RUNTIME_DIR is unset or not absolute",
                        caller.data().name,
                        socket_name
                    );
                    return -1;
                }
            };
            use std::io::Write;
            use std::os::unix::net::UnixStream;

            let mut stream = match UnixStream::connect(&path) {
                Ok(s) => s,
                Err(e) => {
                    let code = e.raw_os_error().unwrap_or(404);
                    debug!(
                        "host_unix_socket_subscribe connect to {:?} failed: {} (code {})",
                        path, e, code
                    );
                    return -code;
                }
            };

            if stream.write_all(b"{\"cmd\":\"subscribe\"}\n").is_err() {
                debug!("host_unix_socket_subscribe handshake to {:?} failed", path);
                return -4;
            }

            let handle =
                NEXT_SUBSCRIPTION_HANDLE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let event_tx = caller.data().socket_event_tx.clone();
            let is_shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let state_stream = match stream.try_clone() {
                Ok(stream) => stream,
                Err(error) => {
                    warn!(
                        "Failed to track Unix socket subscription {}: {}",
                        handle, error
                    );
                    return -5;
                }
            };
            let close_stream = match stream.try_clone() {
                Ok(stream) => stream,
                Err(error) => {
                    warn!(
                        "Failed to register Unix socket subscription {}: {}",
                        handle, error
                    );
                    return -5;
                }
            };
            caller
                .data_mut()
                .active_sockets
                .insert(handle, state_stream);
            lock_unpoisoned(&ACTIVE_SUBSCRIPTIONS)
                .insert(handle, (is_shutdown.clone(), Some(close_stream)));

            std::thread::spawn(move || {
                run_socket_reader(handle, stream, event_tx, is_shutdown);
                lock_unpoisoned(&ACTIVE_SUBSCRIPTIONS).remove(&handle);
            });
            handle as i32
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_unix_socket_close",
        |mut caller: Caller<'_, ModuleHostState>, handle: u32| -> i32 {
            let result = close_subscription(handle);
            if let Some(stream) = caller.data_mut().active_sockets.remove(&handle) {
                shutdown_socket(&stream, handle);
                if result == -1 {
                    return 0;
                }
            }
            result
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_schedule_timer",
        |mut caller: Caller<'_, ModuleHostState>, delay_ms: u64| -> u32 {
            if delay_ms == 0 {
                return 0;
            }
            if caller.data().timer_event_tx.is_closed() {
                return 0;
            }
            let timer_id = NEXT_TIMER_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let is_cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let module_name = caller.data().name.clone();
            ACTIVE_TIMERS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(timer_id, (is_cancelled.clone(), module_name));
            caller.data_mut().active_timers.insert(timer_id);
            let tx = caller.data().timer_event_tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                if !is_cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                    let _ = tx.send(timer_id).await;
                }
                lock_unpoisoned(&ACTIVE_TIMERS).remove(&timer_id);
            });
            timer_id
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_cancel_timer",
        |mut caller: Caller<'_, ModuleHostState>, timer_id: u32| -> i32 {
            caller.data_mut().active_timers.remove(&timer_id);
            if let Some((is_cancelled, _)) = lock_unpoisoned(&ACTIVE_TIMERS).remove(&timer_id) {
                is_cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
                0
            } else {
                -1
            }
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_watch_path",
        |mut caller: Caller<'_, ModuleHostState>, path_ptr: u32, path_len: u32| -> u32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return 0,
            };

            let path_str = match GuestExports::read_string(&memory, &caller, path_ptr, path_len) {
                Ok(s) => s,
                Err(_) => return 0,
            };

            if !caller.data().manifest.can_watch_path(&path_str) {
                warn!(
                    "WASM module '{}' attempted to watch path '{}' without capability",
                    caller.data().name,
                    path_str
                );
                return 0;
            }

            if caller.data().path_event_tx.is_closed() {
                return 0;
            }

            let target_path = std::path::PathBuf::from(&path_str);
            let parent_dir = match target_path.parent() {
                Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
                _ => std::path::PathBuf::from("."),
            };

            let mut curr = parent_dir.clone();
            while !curr.exists() {
                if curr == std::path::Path::new("/") || curr.as_os_str().is_empty() {
                    break;
                }
                if let Some(p) = curr.parent() {
                    if p == curr || p.as_os_str().is_empty() {
                        break;
                    }
                    curr = p.to_path_buf();
                } else {
                    break;
                }
            }

            if !curr.exists() || curr == std::path::Path::new("/") || curr.as_os_str().is_empty() {
                warn!(
                    "WASM module '{}' watch_path '{}' refused: cannot watch root filesystem or non-existent path",
                    caller.data().name,
                    path_str
                );
                return 0;
            }

            if let Ok(canon_curr) = curr.canonicalize() {
                if canon_curr == std::path::Path::new("/") {
                    warn!(
                        "WASM module '{}' watch_path '{}' refused: ancestor canonicalizes to root filesystem",
                        caller.data().name,
                        path_str
                    );
                    return 0;
                }
            }

            let target_file_name = match target_path.file_name().and_then(|f| f.to_str()) {
                Some(f) => f.to_string(),
                None => return 0,
            };

            let watch_id = NEXT_PATH_WATCH_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let tx = caller.data().path_event_tx.clone();
            let module_name = caller.data().name.clone();

            use notify::{Watcher, RecursiveMode, EventKind};

            let is_cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let is_cancelled_cb = is_cancelled.clone();

            let target_fname = target_file_name.clone();
            let target_p = target_path.clone();
            let target_parent_dir = parent_dir.clone();
            let watched_dir = Arc::new(Mutex::new(curr.clone()));
            let watched_dir_cb = watched_dir.clone();

            let watcher_holder: Arc<Mutex<Option<notify::RecommendedWatcher>>> = Arc::new(Mutex::new(None));
            let watcher_weak = Arc::downgrade(&watcher_holder);

            let watcher_res = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                if is_cancelled_cb.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                if let Ok(event) = res {
                    let current_active_dir = lock_unpoisoned(&watched_dir_cb).clone();
                    if !event.paths.iter().any(|p| p.starts_with(&current_active_dir)) {
                        return;
                    }

                    let appeared = matches!(
                        event.kind,
                        EventKind::Create(_)
                            | EventKind::Modify(notify::event::ModifyKind::Name(
                                notify::event::RenameMode::To
                            ))
                            | EventKind::Modify(notify::event::ModifyKind::Name(
                                notify::event::RenameMode::Any
                            ))
                    );
                    if appeared {
                        // 1. Check if the target socket/file itself was created
                        let matches_target = event.paths.iter().any(|p| {
                            p == &target_p || (p.file_name().and_then(|f| f.to_str()) == Some(&target_fname) && target_p.starts_with(p.parent().unwrap_or(std::path::Path::new(""))))
                        });
                        if matches_target {
                            if !is_cancelled_cb.load(std::sync::atomic::Ordering::SeqCst) {
                                let _ = tx.try_send(watch_id);
                            }
                            return;
                        }

                        // 2. Check if an intermediate ancestor directory was created along target_path
                        let intermediate_created = event.paths.iter().any(|p| {
                            target_p.starts_with(p) && p != &target_p
                        });

                        if intermediate_created {
                            // Find the deepest existing ancestor towards target_parent_dir
                            let mut deepest = target_parent_dir.clone();
                            while !deepest.exists() {
                                if let Some(p) = deepest.parent() {
                                    deepest = p.to_path_buf();
                                } else {
                                    break;
                                }
                            }

                            let current_dir_val = lock_unpoisoned(&watched_dir_cb).clone();
                            if deepest.exists() && deepest != current_dir_val {
                                let watcher_weak = watcher_weak.clone();
                                let watched_dir_cb = watched_dir_cb.clone();
                                let deepest_clone = deepest.clone();
                                let tx_clone = tx.clone();
                                let target_p_clone = target_p.clone();
                                let is_cancelled_worker = is_cancelled_cb.clone();
                                let target_parent_dir_thread = target_parent_dir.clone();
                                std::thread::spawn(move || {
                                    struct EscalationGuard;
                                    impl Drop for EscalationGuard {
                                        fn drop(&mut self) {
                                            ACTIVE_ESCALATION_THREADS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                                        }
                                    }
                                    ACTIVE_ESCALATION_THREADS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                    let _guard = EscalationGuard;

                                    if is_cancelled_worker.load(std::sync::atomic::Ordering::SeqCst) {
                                        return;
                                    }
                                    // Re-evaluate deepest existing directory in case further subdirectories were created
                                    let mut target_dir = target_parent_dir_thread.clone();
                                    while !target_dir.exists() {
                                        if is_cancelled_worker.load(std::sync::atomic::Ordering::SeqCst) {
                                            return;
                                        }
                                        if let Some(p) = target_dir.parent() {
                                            target_dir = p.to_path_buf();
                                        } else {
                                            break;
                                        }
                                    }
                                    if !target_dir.exists() {
                                        target_dir = deepest_clone;
                                    }

                                    if is_cancelled_worker.load(std::sync::atomic::Ordering::SeqCst) {
                                        return;
                                    }

                                    if let Some(holder) = watcher_weak.upgrade() {
                                        let mut guard = lock_unpoisoned(&holder);
                                        if let Some(w) = guard.as_mut() {
                                            if target_dir != current_dir_val
                                                && target_dir.exists()
                                                && w.watch(&target_dir, RecursiveMode::NonRecursive).is_ok()
                                            {
                                                let _ = w.unwatch(&current_dir_val);
                                                *lock_unpoisoned(&watched_dir_cb) = target_dir;
                                            }
                                        }
                                    }
                                    if !is_cancelled_worker.load(std::sync::atomic::Ordering::SeqCst) && target_p_clone.exists() {
                                        let _ = tx_clone.try_send(watch_id);
                                    }
                                });
                            }

                            if !is_cancelled_cb.load(std::sync::atomic::Ordering::SeqCst) && target_p.exists() {
                                let _ = tx.try_send(watch_id);
                            }
                        }
                    }
                }
            });

            let mut watcher = match watcher_res {
                Ok(w) => w,
                Err(e) => {
                    error!("Failed to create inotify watcher for '{}': {}", path_str, e);
                    return 0;
                }
            };

            if let Err(e) = watcher.watch(&curr, RecursiveMode::NonRecursive) {
                error!("Failed to watch ancestor dir '{:?}': {}", curr, e);
                return 0;
            }

            *lock_unpoisoned(&watcher_holder) = Some(watcher);

            lock_unpoisoned(&ACTIVE_PATH_WATCHES).insert(
                watch_id,
                PathWatchEntry {
                    _watcher: watcher_holder,
                    module_name,
                    target_path,
                    is_cancelled,
                },
            );
            caller.data_mut().active_watches.insert(watch_id);

            watch_id
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_unwatch_path",
        |mut caller: Caller<'_, ModuleHostState>, watch_id: u32| -> i32 {
            caller.data_mut().active_watches.remove(&watch_id);
            if let Some(entry) = lock_unpoisoned(&ACTIVE_PATH_WATCHES).remove(&watch_id) {
                entry
                    .is_cancelled
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                let _ = lock_unpoisoned(&entry._watcher).take();
                0
            } else {
                -1
            }
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_get_env",
        |mut caller: Caller<'_, ModuleHostState>,
         key_ptr: u32,
         key_len: u32,
         out_ptr: u32,
         out_max_len: u32|
         -> i32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };

            let key = match GuestExports::read_string(&memory, &caller, key_ptr, key_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };

            if !caller.data().manifest.can_read_env(&key) {
                log::warn!(
                    "Module '{}' denied access to env var '{}' (missing env:read capability)",
                    caller.data().name,
                    key
                );
                return -4;
            }

            let val = match std::env::var(&key) {
                Ok(v) => v,
                Err(_) => return 0,
            };

            let bytes = val.as_bytes();
            let len = bytes.len().min(out_max_len as usize);
            let mem_data = memory.data_mut(&mut caller);
            let start = out_ptr as usize;
            if start + len <= mem_data.len() {
                mem_data[start..start + len].copy_from_slice(&bytes[..len]);
                len as i32
            } else {
                -3
            }
        },
    )?;

    linker.func_wrap(
        "wyrd_host",
        "host_resolve_icon",
        |mut caller: Caller<'_, ModuleHostState>,
         name_ptr: u32,
         name_len: u32,
         out_ptr: u32,
         out_max_len: u32|
         -> i32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };

            let name = match GuestExports::read_string(&memory, &caller, name_ptr, name_len) {
                Ok(s) => s,
                Err(_) => return -2,
            };

            if let Some(path) = crate::icons::resolve_icon(&name) {
                let bytes = path.as_bytes();
                let len = bytes.len().min(out_max_len as usize);
                let mem_data = memory.data_mut(&mut caller);
                let start = out_ptr as usize;
                if start + len <= mem_data.len() {
                    mem_data[start..start + len].copy_from_slice(&bytes[..len]);
                    len as i32
                } else {
                    -3
                }
            } else {
                -4
            }
        },
    )?;

    Ok(())
}

fn json_to_zvariant(val: &serde_json::Value) -> zbus::zvariant::Value<'static> {
    match val {
        serde_json::Value::Null => zbus::zvariant::Value::Str("".into()),
        serde_json::Value::Bool(b) => zbus::zvariant::Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                zbus::zvariant::Value::I64(i)
            } else if let Some(u) = n.as_u64() {
                zbus::zvariant::Value::U64(u)
            } else if let Some(f) = n.as_f64() {
                zbus::zvariant::Value::F64(f)
            } else {
                zbus::zvariant::Value::from(0i32)
            }
        }
        serde_json::Value::String(s) => zbus::zvariant::Value::from(s.clone()),
        serde_json::Value::Array(arr) => {
            let zvs: Vec<zbus::zvariant::Value<'static>> =
                arr.iter().map(json_to_zvariant).collect();
            zbus::zvariant::Value::from(zvs)
        }
        serde_json::Value::Object(obj) => {
            let map: std::collections::HashMap<String, zbus::zvariant::Value<'static>> = obj
                .iter()
                .map(|(k, v)| (k.clone(), json_to_zvariant(v)))
                .collect();
            zbus::zvariant::Value::from(map)
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum DbusArg {
    Unit,
    Bool(bool),
    U8(u8),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F64(f64),
    Str(String),
    ObjectPath(OwnedObjectPath),
    Signature(Signature),
    ArrayStr(Vec<String>),
    ArrayObjectPath(Vec<OwnedObjectPath>),
    ArrayU8(Vec<u8>),
    ArrayU32(Vec<u32>),
    ArrayI32(Vec<i32>),
    DictStrVariant(HashMap<String, Value<'static>>),
    DictStrStr(HashMap<String, String>),
    DictStrDict(HashMap<String, HashMap<String, Value<'static>>>),
    ArrayDictStrVariant(Vec<HashMap<String, Value<'static>>>),
    Variant(Value<'static>),
}

impl serde::Serialize for DbusArg {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            DbusArg::Unit => ().serialize(serializer),
            DbusArg::Bool(b) => b.serialize(serializer),
            DbusArg::U8(u) => u.serialize(serializer),
            DbusArg::I16(i) => i.serialize(serializer),
            DbusArg::U16(u) => u.serialize(serializer),
            DbusArg::I32(i) => i.serialize(serializer),
            DbusArg::U32(u) => u.serialize(serializer),
            DbusArg::I64(i) => i.serialize(serializer),
            DbusArg::U64(u) => u.serialize(serializer),
            DbusArg::F64(f) => f.serialize(serializer),
            DbusArg::Str(s) => s.serialize(serializer),
            DbusArg::ObjectPath(p) => p.serialize(serializer),
            DbusArg::Signature(s) => s.serialize(serializer),
            DbusArg::ArrayStr(arr) => arr.serialize(serializer),
            DbusArg::ArrayObjectPath(arr) => arr.serialize(serializer),
            DbusArg::ArrayU8(arr) => arr.serialize(serializer),
            DbusArg::ArrayU32(arr) => arr.serialize(serializer),
            DbusArg::ArrayI32(arr) => arr.serialize(serializer),
            DbusArg::DictStrVariant(d) => d.serialize(serializer),
            DbusArg::DictStrStr(d) => d.serialize(serializer),
            DbusArg::DictStrDict(d) => d.serialize(serializer),
            DbusArg::ArrayDictStrVariant(arr) => arr.serialize(serializer),
            DbusArg::Variant(v) => v.serialize(serializer),
        }
    }
}

impl DynamicType for DbusArg {
    fn signature(&self) -> Signature {
        match self {
            DbusArg::Unit => Signature::Unit,
            DbusArg::Bool(_) => bool::SIGNATURE.clone(),
            DbusArg::U8(_) => u8::SIGNATURE.clone(),
            DbusArg::I16(_) => i16::SIGNATURE.clone(),
            DbusArg::U16(_) => u16::SIGNATURE.clone(),
            DbusArg::I32(_) => i32::SIGNATURE.clone(),
            DbusArg::U32(_) => u32::SIGNATURE.clone(),
            DbusArg::I64(_) => i64::SIGNATURE.clone(),
            DbusArg::U64(_) => u64::SIGNATURE.clone(),
            DbusArg::F64(_) => f64::SIGNATURE.clone(),
            DbusArg::Str(_) => <&str>::SIGNATURE.clone(),
            DbusArg::ObjectPath(_) => <OwnedObjectPath as Type>::SIGNATURE.clone(),
            DbusArg::Signature(_) => <Signature as Type>::SIGNATURE.clone(),
            DbusArg::ArrayStr(_) => <Vec<String> as Type>::SIGNATURE.clone(),
            DbusArg::ArrayObjectPath(_) => <Vec<OwnedObjectPath> as Type>::SIGNATURE.clone(),
            DbusArg::ArrayU8(_) => <Vec<u8> as Type>::SIGNATURE.clone(),
            DbusArg::ArrayU32(_) => <Vec<u32> as Type>::SIGNATURE.clone(),
            DbusArg::ArrayI32(_) => <Vec<i32> as Type>::SIGNATURE.clone(),
            DbusArg::DictStrVariant(_) => Signature::try_from("a{sv}").unwrap_or(Signature::Unit),
            DbusArg::DictStrStr(_) => Signature::try_from("a{ss}").unwrap_or(Signature::Unit),
            DbusArg::DictStrDict(_) => Signature::try_from("a{sa{sv}}").unwrap_or(Signature::Unit),
            DbusArg::ArrayDictStrVariant(_) => {
                Signature::try_from("aa{sv}").unwrap_or(Signature::Unit)
            }
            DbusArg::Variant(_) => Signature::Variant,
        }
    }
}

type MethodCacheKey = (String, String, String, String, String);
type MethodCacheVal = (String, Vec<String>, Option<std::time::Instant>);

static METHOD_SIG_CACHE: LazyLock<Mutex<HashMap<MethodCacheKey, MethodCacheVal>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn get_or_introspect_method(
    conn: &zbus::blocking::Connection,
    bus: &str,
    service: &str,
    path: &str,
    iface: &str,
    member: &str,
) -> (String, Vec<String>) {
    let key = (
        bus.to_string(),
        service.to_string(),
        path.to_string(),
        iface.to_string(),
        member.to_string(),
    );

    if let Ok(guard) = METHOD_SIG_CACHE.lock() {
        if let Some((cached_iface, cached_args, neg_ts)) = guard.get(&key) {
            if neg_ts.is_none_or(|ts| ts.elapsed() < std::time::Duration::from_secs(30)) {
                return (cached_iface.clone(), cached_args.clone());
            }
        }
    }

    let dest = if service.is_empty() {
        None
    } else {
        Some(service)
    };
    let introspect_res = conn
        .call_method(
            dest,
            path,
            Some("org.freedesktop.DBus.Introspectable"),
            "Introspect",
            &(),
        )
        .and_then(|reply| reply.body().deserialize::<String>());

    if let Ok(xml) = introspect_res {
        if let Some((discovered_iface, in_args)) = parse_introspect_xml(&xml, iface, member) {
            if let Ok(mut guard) = METHOD_SIG_CACHE.lock() {
                guard.insert(key, (discovered_iface.clone(), in_args.clone(), None));
            }
            return (discovered_iface, in_args);
        }
    }

    if let Ok(mut guard) = METHOD_SIG_CACHE.lock() {
        guard.insert(
            key,
            (
                iface.to_string(),
                Vec::new(),
                Some(std::time::Instant::now()),
            ),
        );
    }
    (iface.to_string(), Vec::new())
}

fn parse_introspect_xml(
    xml: &str,
    target_iface: &str,
    target_member: &str,
) -> Option<(String, Vec<String>)> {
    let mut current_iface: Option<String> = None;
    let mut current_method: Option<String> = None;
    let mut in_args: Vec<String> = Vec::new();
    let mut found_result: Option<(String, Vec<String>)> = None;

    let mut pos = 0;
    while let Some(tag_start) = xml[pos..].find('<') {
        let abs_start = pos + tag_start;
        let Some(tag_end) = xml[abs_start..].find('>') else {
            break;
        };
        let abs_end = abs_start + tag_end;
        let tag_content = xml[abs_start + 1..abs_end].trim();
        pos = abs_end + 1;

        if tag_content.starts_with("!--") {
            continue;
        }

        if tag_content.starts_with("interface ") {
            if let Some(name) = extract_xml_attr(tag_content, "name") {
                current_iface = Some(name);
            }
        } else if tag_content == "/interface" {
            current_iface = None;
        } else if tag_content.starts_with("method ") {
            if let Some(name) = extract_xml_attr(tag_content, "name") {
                current_method = Some(name);
                in_args.clear();
            }
            if tag_content.ends_with('/') {
                if let (Some(ref iface), Some(ref method)) = (&current_iface, &current_method) {
                    let iface_matches = target_iface.is_empty() || iface == target_iface;
                    if iface_matches && method == target_member {
                        found_result = Some((iface.clone(), in_args.clone()));
                        break;
                    }
                }
                current_method = None;
                in_args.clear();
            }
        } else if tag_content == "/method" {
            if let (Some(ref iface), Some(ref method)) = (&current_iface, &current_method) {
                let iface_matches = target_iface.is_empty() || iface == target_iface;
                if iface_matches && method == target_member {
                    found_result = Some((iface.clone(), in_args.clone()));
                    break;
                }
            }
            current_method = None;
            in_args.clear();
        } else if tag_content.starts_with("arg ") && current_method.is_some() {
            let dir =
                extract_xml_attr(tag_content, "direction").unwrap_or_else(|| "in".to_string());
            if dir == "in" {
                if let Some(sig) = extract_xml_attr(tag_content, "type") {
                    in_args.push(sig);
                }
            }
        }
    }

    found_result
}

fn extract_xml_attr(tag: &str, attr_name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let pattern = format!("{}=", attr_name);
        if let Some(idx) = tag.find(&pattern) {
            let after = tag[idx + pattern.len()..].trim_start();
            if after.starts_with(quote) {
                let rest = &after[quote.len_utf8()..];
                if let Some(end_idx) = rest.find(quote) {
                    return Some(rest[..end_idx].to_string());
                }
            }
        }
    }
    None
}

fn json_to_dict_str_variant(val: &serde_json::Value) -> HashMap<String, Value<'static>> {
    let mut map = HashMap::new();
    if let Some(obj) = val.as_object() {
        for (k, v) in obj {
            map.insert(k.clone(), json_to_zvariant(v));
        }
    }
    map
}

fn json_to_dict_str_dict(
    val: &serde_json::Value,
) -> HashMap<String, HashMap<String, Value<'static>>> {
    let mut map = HashMap::new();
    if let Some(obj) = val.as_object() {
        for (section, props) in obj {
            map.insert(section.clone(), json_to_dict_str_variant(props));
        }
    }
    map
}

fn json_to_dbus_arg(val: &serde_json::Value, expected_sig: Option<&str>) -> DbusArg {
    if let Some(sig) = expected_sig {
        match sig {
            "b" => {
                return DbusArg::Bool(
                    val.as_bool()
                        .unwrap_or_else(|| val.as_i64().map(|n| n != 0).unwrap_or(false)),
                )
            }
            "y" => return DbusArg::U8(val.as_u64().unwrap_or(0) as u8),
            "n" => return DbusArg::I16(val.as_i64().unwrap_or(0) as i16),
            "q" => return DbusArg::U16(val.as_u64().unwrap_or(0) as u16),
            "i" => return DbusArg::I32(val.as_i64().unwrap_or(0) as i32),
            "u" => return DbusArg::U32(val.as_u64().unwrap_or(0) as u32),
            "x" => return DbusArg::I64(val.as_i64().unwrap_or(0)),
            "t" => return DbusArg::U64(val.as_u64().unwrap_or(0)),
            "d" => return DbusArg::F64(val.as_f64().unwrap_or(0.0)),
            "s" => {
                return DbusArg::Str(
                    val.as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| val.to_string()),
                )
            }
            "o" => {
                let s = val.as_str().unwrap_or("/");
                let path =
                    OwnedObjectPath::try_from(s).unwrap_or_else(|_| OwnedObjectPath::default());
                return DbusArg::ObjectPath(path);
            }
            "g" => {
                let s = val.as_str().unwrap_or("");
                let sig = Signature::try_from(s).unwrap_or(Signature::Unit);
                return DbusArg::Signature(sig);
            }
            "as" => {
                let list = val
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .map(|item| {
                                item.as_str()
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| item.to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                return DbusArg::ArrayStr(list);
            }
            "ao" => {
                let list = val
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|item| {
                                let s = item.as_str().unwrap_or("/");
                                OwnedObjectPath::try_from(s).ok()
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                return DbusArg::ArrayObjectPath(list);
            }
            "ay" => {
                let bytes = if let Some(arr) = val.as_array() {
                    arr.iter().map(|b| b.as_u64().unwrap_or(0) as u8).collect()
                } else if let Some(s) = val.as_str() {
                    s.as_bytes().to_vec()
                } else {
                    Vec::new()
                };
                return DbusArg::ArrayU8(bytes);
            }
            "au" => {
                let list = val
                    .as_array()
                    .map(|arr| arr.iter().map(|b| b.as_u64().unwrap_or(0) as u32).collect())
                    .unwrap_or_default();
                return DbusArg::ArrayU32(list);
            }
            "ai" => {
                let list = val
                    .as_array()
                    .map(|arr| arr.iter().map(|b| b.as_i64().unwrap_or(0) as i32).collect())
                    .unwrap_or_default();
                return DbusArg::ArrayI32(list);
            }
            "a{sv}" => {
                return DbusArg::DictStrVariant(json_to_dict_str_variant(val));
            }
            "a{ss}" => {
                let mut map = HashMap::new();
                if let Some(obj) = val.as_object() {
                    for (k, v) in obj {
                        map.insert(
                            k.clone(),
                            v.as_str()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| v.to_string()),
                        );
                    }
                }
                return DbusArg::DictStrStr(map);
            }
            "a{sa{sv}}" => {
                return DbusArg::DictStrDict(json_to_dict_str_dict(val));
            }
            "aa{sv}" => {
                let list = val
                    .as_array()
                    .map(|arr| arr.iter().map(json_to_dict_str_variant).collect())
                    .unwrap_or_default();
                return DbusArg::ArrayDictStrVariant(list);
            }
            "v" => {
                return DbusArg::Variant(json_to_zvariant(val));
            }
            _ => {}
        }
    }

    match val {
        serde_json::Value::Null => DbusArg::Unit,
        serde_json::Value::Bool(b) => DbusArg::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                if i >= i32::MIN as i64 && i <= i32::MAX as i64 {
                    DbusArg::I32(i as i32)
                } else {
                    DbusArg::I64(i)
                }
            } else if let Some(u) = n.as_u64() {
                if u <= u32::MAX as u64 {
                    DbusArg::U32(u as u32)
                } else {
                    DbusArg::U64(u)
                }
            } else if let Some(f) = n.as_f64() {
                DbusArg::F64(f)
            } else {
                DbusArg::I32(0)
            }
        }
        serde_json::Value::String(s) => {
            if s.starts_with('/') && !s.contains('.') {
                if let Ok(path) = OwnedObjectPath::try_from(s.as_str()) {
                    return DbusArg::ObjectPath(path);
                }
            }
            DbusArg::Str(s.clone())
        }
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                DbusArg::ArrayStr(Vec::new())
            } else if arr.iter().all(|item| item.is_string()) {
                let list = arr
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect();
                DbusArg::ArrayStr(list)
            } else if arr.iter().all(|item| item.is_u64()) {
                let bytes: Vec<u8> = arr
                    .iter()
                    .filter_map(|item| item.as_u64().map(|n| n as u8))
                    .collect();
                DbusArg::ArrayU8(bytes)
            } else if arr.iter().all(|item| item.is_object()) {
                let list = arr.iter().map(json_to_dict_str_variant).collect();
                DbusArg::ArrayDictStrVariant(list)
            } else {
                let zvs: Vec<Value<'static>> = arr.iter().map(json_to_zvariant).collect();
                DbusArg::Variant(Value::from(zvs))
            }
        }
        serde_json::Value::Object(obj) => {
            if obj.is_empty() {
                DbusArg::DictStrVariant(HashMap::new())
            } else if obj.values().all(|v| v.is_object()) {
                DbusArg::DictStrDict(json_to_dict_str_dict(val))
            } else {
                DbusArg::DictStrVariant(json_to_dict_str_variant(val))
            }
        }
    }
}

static SYSTEM_CONN: LazyLock<Mutex<Option<zbus::blocking::Connection>>> =
    LazyLock::new(|| Mutex::new(None));
static SESSION_CONN: LazyLock<Mutex<Option<zbus::blocking::Connection>>> =
    LazyLock::new(|| Mutex::new(None));

pub(crate) fn get_blocking_dbus_conn(bus: &str) -> Result<zbus::blocking::Connection> {
    let mutex = if bus == "system" {
        &SYSTEM_CONN
    } else {
        &SESSION_CONN
    };

    let mut lock = mutex.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(conn) = lock.as_ref() {
        return Ok(conn.clone());
    }

    let conn = if bus == "system" {
        zbus::blocking::Connection::system()?
    } else {
        zbus::blocking::Connection::session()?
    };
    *lock = Some(conn.clone());
    Ok(conn)
}

pub(crate) fn reset_blocking_dbus_conn(bus: &str) {
    let mutex = if bus == "system" {
        &SYSTEM_CONN
    } else {
        &SESSION_CONN
    };
    if let Ok(mut lock) = mutex.lock() {
        *lock = None;
    }
}

pub(crate) fn perform_dbus_call(
    bus: &str,
    service: &str,
    path: &str,
    iface: &str,
    member: &str,
    args_json: &str,
) -> Result<String> {
    let conn = match get_blocking_dbus_conn(bus) {
        Ok(c) => c,
        Err(_) => {
            reset_blocking_dbus_conn(bus);
            get_blocking_dbus_conn(bus)?
        }
    };

    let args_val: serde_json::Value = if args_json.trim().is_empty() {
        serde_json::Value::Array(Vec::new())
    } else {
        serde_json::from_str(args_json).unwrap_or(serde_json::Value::Array(Vec::new()))
    };

    // Fast path for Properties.Get and Properties.GetAll without creating a caching zbus::Proxy
    let dest_opt = if service.is_empty() {
        None
    } else {
        Some(service)
    };
    if (iface == "org.freedesktop.DBus.Properties" || iface.is_empty()) && member == "Get" {
        if let serde_json::Value::Array(ref arr) = args_val {
            if arr.len() >= 2 {
                let target_iface = arr[0].as_str().unwrap_or("");
                let prop_name = arr[1].as_str().unwrap_or("");
                let reply = conn.call_method(
                    dest_opt,
                    path,
                    Some("org.freedesktop.DBus.Properties"),
                    "Get",
                    &(target_iface, prop_name),
                )?;
                let val: zbus::zvariant::OwnedValue = reply.body().deserialize()?;
                let json_val = zvariant_to_json(&val);
                return Ok(serde_json::to_string(&json_val)?);
            }
        }
    } else if (iface == "org.freedesktop.DBus.Properties" || iface.is_empty()) && member == "GetAll"
    {
        if let serde_json::Value::Array(ref arr) = args_val {
            if let Some(target_iface) = arr.first().and_then(|v| v.as_str()) {
                let reply = conn.call_method(
                    dest_opt,
                    path,
                    Some("org.freedesktop.DBus.Properties"),
                    "GetAll",
                    &(target_iface,),
                )?;
                let map: std::collections::HashMap<String, zbus::zvariant::OwnedValue> =
                    reply.body().deserialize()?;
                let mut json_map = serde_json::Map::new();
                for (k, v) in map {
                    json_map.insert(k, zvariant_to_json(&v));
                }
                return Ok(serde_json::to_string(&serde_json::Value::Object(json_map))?);
            }
        }
    } else if (iface == "org.freedesktop.DBus.Properties" || iface.is_empty()) && member == "Set" {
        if let serde_json::Value::Array(ref arr) = args_val {
            if arr.len() >= 3 {
                let target_iface = arr[0].as_str().unwrap_or("");
                let prop_name = arr[1].as_str().unwrap_or("");
                let val = json_to_zvariant(&arr[2]);
                let _ = conn.call_method(
                    dest_opt,
                    path,
                    Some("org.freedesktop.DBus.Properties"),
                    "Set",
                    &(target_iface, prop_name, val),
                )?;
                return Ok("{}".to_string());
            }
        }
    }

    // Generic dynamic D-Bus call dispatch (service-agnostic)
    let args_slice = match &args_val {
        serde_json::Value::Array(arr) => arr.as_slice(),
        _ => std::slice::from_ref(&args_val),
    };

    let (discovered_iface, in_signatures) =
        get_or_introspect_method(&conn, bus, service, path, iface, member);
    let target_iface = if !iface.is_empty() {
        Some(iface)
    } else if !discovered_iface.is_empty() {
        Some(discovered_iface.as_str())
    } else {
        None
    };

    let mut dbus_args: Vec<DbusArg> = Vec::with_capacity(args_slice.len());
    for (i, arg_val) in args_slice.iter().enumerate() {
        let expected_sig = in_signatures.get(i).map(|s| s.as_str());
        dbus_args.push(json_to_dbus_arg(arg_val, expected_sig));
    }

    let dest = if service.is_empty() {
        None
    } else {
        Some(service)
    };
    macro_rules! call_dynamic_tuple {
        ($($idx:expr),*) => {{
            let mut it = dbus_args.into_iter();
            conn.call_method(
                dest,
                path,
                target_iface,
                member,
                &DynamicTuple(($( {
                    let _ = $idx;
                    it.next().unwrap_or(DbusArg::Unit)
                }, )*)),
            )?
        }};
    }

    let reply = match dbus_args.len() {
        0 => conn.call_method(dest, path, target_iface, member, &())?,
        1 => call_dynamic_tuple!(0),
        2 => call_dynamic_tuple!(0, 1),
        3 => call_dynamic_tuple!(0, 1, 2),
        4 => call_dynamic_tuple!(0, 1, 2, 3),
        5 => call_dynamic_tuple!(0, 1, 2, 3, 4),
        6 => call_dynamic_tuple!(0, 1, 2, 3, 4, 5),
        7 => call_dynamic_tuple!(0, 1, 2, 3, 4, 5, 6),
        8 => call_dynamic_tuple!(0, 1, 2, 3, 4, 5, 6, 7),
        _ => {
            let zvs: Vec<Value<'static>> = args_slice.iter().map(json_to_zvariant).collect();
            conn.call_method(dest, path, target_iface, member, &(zvs,))?
        }
    };

    deserialize_dbus_reply_to_json(&reply)
}

pub(crate) fn zvariant_to_json(val: &zbus::zvariant::Value<'_>) -> serde_json::Value {
    use zbus::zvariant::Value;
    match val {
        Value::U8(v) => serde_json::json!(*v),
        Value::Bool(v) => serde_json::json!(*v),
        Value::I16(v) => serde_json::json!(*v),
        Value::U16(v) => serde_json::json!(*v),
        Value::I32(v) => serde_json::json!(*v),
        Value::U32(v) => serde_json::json!(*v),
        Value::I64(v) => serde_json::json!(*v),
        Value::U64(v) => serde_json::json!(*v),
        Value::F64(v) => serde_json::json!(*v),
        Value::Str(v) => serde_json::json!(v.as_str()),
        Value::Signature(v) => serde_json::json!(v.to_string()),
        Value::ObjectPath(v) => serde_json::json!(v.as_str()),
        Value::Value(v) => zvariant_to_json(v),
        Value::Array(arr) => {
            if arr.len() > 128
                && arr
                    .iter()
                    .next()
                    .is_some_and(|item| matches!(item, Value::U8(_)))
            {
                let items: Vec<serde_json::Value> =
                    arr.iter().take(32).map(zvariant_to_json).collect();
                serde_json::Value::Array(items)
            } else {
                let items: Vec<serde_json::Value> = arr.iter().map(zvariant_to_json).collect();
                serde_json::Value::Array(items)
            }
        }
        Value::Dict(dict) => {
            let mut map = serde_json::Map::new();
            for (k, v) in dict.iter() {
                let key_str = match k {
                    Value::Str(s) => s.as_str().to_string(),
                    Value::ObjectPath(p) => p.as_str().to_string(),
                    other => format!("{:?}", other),
                };
                map.insert(key_str, zvariant_to_json(v));
            }
            serde_json::Value::Object(map)
        }
        Value::Structure(s) => {
            let items: Vec<serde_json::Value> = s.fields().iter().map(zvariant_to_json).collect();
            serde_json::Value::Array(items)
        }
        _ => serde_json::Value::Null,
    }
}

fn deserialize_dbus_reply_to_json(reply: &zbus::message::Message) -> Result<String> {
    let body = reply.body();
    let sig = body.signature();
    let sig_str = sig.to_string();

    if sig_str.starts_with('(') && sig_str.ends_with(')') {
        if let Ok(st) = body.deserialize::<zbus::zvariant::Structure>() {
            let items: Vec<serde_json::Value> = st.fields().iter().map(zvariant_to_json).collect();
            return Ok(serde_json::to_string(&serde_json::Value::Array(items))?);
        }
    }

    match sig_str.as_str() {
        "v" => {
            let val: zbus::zvariant::OwnedValue = body.deserialize()?;
            let json_val = zvariant_to_json(&val);
            Ok(serde_json::to_string(&json_val)?)
        }
        "s" | "o" | "g" => {
            let s: String = body.deserialize()?;
            Ok(serde_json::to_string(&s)?)
        }
        "b" => {
            let b: bool = body.deserialize()?;
            Ok(serde_json::to_string(&b)?)
        }
        "y" => {
            let y: u8 = body.deserialize()?;
            Ok(serde_json::to_string(&y)?)
        }
        "n" => {
            let n: i16 = body.deserialize()?;
            Ok(serde_json::to_string(&n)?)
        }
        "q" => {
            let q: u16 = body.deserialize()?;
            Ok(serde_json::to_string(&q)?)
        }
        "i" => {
            let i: i32 = body.deserialize()?;
            Ok(serde_json::to_string(&i)?)
        }
        "u" => {
            let u: u32 = body.deserialize()?;
            Ok(serde_json::to_string(&u)?)
        }
        "x" => {
            let x: i64 = body.deserialize()?;
            Ok(serde_json::to_string(&x)?)
        }
        "t" => {
            let t: u64 = body.deserialize()?;
            Ok(serde_json::to_string(&t)?)
        }
        "d" => {
            let d: f64 = body.deserialize()?;
            Ok(serde_json::to_string(&d)?)
        }
        "as" => {
            let list: Vec<String> = body.deserialize()?;
            Ok(serde_json::to_string(&list)?)
        }
        "ao" => {
            let list: Vec<zbus::zvariant::OwnedObjectPath> = body.deserialize()?;
            let str_list: Vec<String> = list.into_iter().map(|p| p.to_string()).collect();
            Ok(serde_json::to_string(&str_list)?)
        }
        "ay" => {
            let bytes: Vec<u8> = body.deserialize()?;
            Ok(serde_json::to_string(&bytes)?)
        }
        "au" => {
            let nums: Vec<u32> = body.deserialize()?;
            Ok(serde_json::to_string(&nums)?)
        }
        "a{sv}" => {
            let map: std::collections::HashMap<String, zbus::zvariant::OwnedValue> =
                body.deserialize()?;
            let mut json_map = serde_json::Map::new();
            for (k, v) in map {
                json_map.insert(k, zvariant_to_json(&v));
            }
            Ok(serde_json::to_string(&serde_json::Value::Object(json_map))?)
        }
        "a{ss}" => {
            let map: std::collections::HashMap<String, String> = body.deserialize()?;
            Ok(serde_json::to_string(&map)?)
        }
        "aa{sv}" => {
            let list: Vec<std::collections::HashMap<String, zbus::zvariant::OwnedValue>> =
                body.deserialize()?;
            let json_list: Vec<serde_json::Value> = list
                .into_iter()
                .map(|map| {
                    let mut json_map = serde_json::Map::new();
                    for (k, v) in map {
                        json_map.insert(k, zvariant_to_json(&v));
                    }
                    serde_json::Value::Object(json_map)
                })
                .collect();
            Ok(serde_json::to_string(&json_list)?)
        }
        "a{sa{sv}}" => {
            let map: std::collections::HashMap<
                String,
                std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
            > = body.deserialize()?;
            let mut json_map = serde_json::Map::new();
            for (section, props) in map {
                let mut section_map = serde_json::Map::new();
                for (k, v) in props {
                    section_map.insert(k, zvariant_to_json(&v));
                }
                json_map.insert(section, serde_json::Value::Object(section_map));
            }
            Ok(serde_json::to_string(&serde_json::Value::Object(json_map))?)
        }
        "a{oa{sa{sv}}}" => {
            let map: std::collections::HashMap<
                zbus::zvariant::OwnedObjectPath,
                std::collections::HashMap<
                    String,
                    std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
                >,
            > = body.deserialize()?;
            let mut json_map = serde_json::Map::new();
            for (path, ifaces) in map {
                let mut ifaces_map = serde_json::Map::new();
                for (iface, props) in ifaces {
                    let mut props_map = serde_json::Map::new();
                    for (k, v) in props {
                        props_map.insert(k, zvariant_to_json(&v));
                    }
                    ifaces_map.insert(iface, serde_json::Value::Object(props_map));
                }
                json_map.insert(
                    path.as_str().to_string(),
                    serde_json::Value::Object(ifaces_map),
                );
            }
            Ok(serde_json::to_string(&serde_json::Value::Object(json_map))?)
        }
        _ => {
            if let Ok(val) = body.deserialize::<zbus::zvariant::OwnedValue>() {
                let json_val = zvariant_to_json(&val);
                Ok(serde_json::to_string(&json_val)?)
            } else if let Ok(s) = body.deserialize::<String>() {
                Ok(serde_json::to_string(&s)?)
            } else {
                Ok("{}".to_string())
            }
        }
    }
}

static ASYNC_SYSTEM_CONN: LazyLock<tokio::sync::Mutex<Option<zbus::Connection>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(None));
static ASYNC_SESSION_CONN: LazyLock<tokio::sync::Mutex<Option<zbus::Connection>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(None));

async fn get_shared_async_dbus_conn(bus: &str) -> Result<zbus::Connection, zbus::Error> {
    let cell = if bus == "system" {
        &ASYNC_SYSTEM_CONN
    } else {
        &ASYNC_SESSION_CONN
    };
    let mut guard = cell.lock().await;
    if let Some(conn) = guard.as_ref() {
        return Ok(conn.clone());
    }
    let conn = if bus == "system" {
        zbus::Connection::system().await?
    } else {
        zbus::Connection::session().await?
    };
    *guard = Some(conn.clone());
    Ok(conn)
}

async fn subscribe_dbus_signal(
    bus: String,
    service: String,
    path: String,
    iface: String,
    member: String,
    signal_tx: mpsc::Sender<(String, String, String, String)>,
) {
    use futures_util::StreamExt;
    let conn = match get_shared_async_dbus_conn(&bus).await {
        Ok(c) => c,
        Err(e) => {
            warn!(
                "Failed to connect to {} bus for signal subscription: {}",
                bus, e
            );
            return;
        }
    };

    let mut builder = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal);
    if !iface.is_empty() {
        if let Ok(i) = zbus::names::InterfaceName::try_from(iface.as_str()) {
            builder = match builder.interface(i) {
                Ok(b) => b,
                Err(e) => {
                    warn!("Invalid interface for MatchRule: {}", e);
                    return;
                }
            };
        }
    }
    if !member.is_empty() {
        if let Ok(m) = zbus::names::MemberName::try_from(member.as_str()) {
            builder = match builder.member(m) {
                Ok(b) => b,
                Err(e) => {
                    warn!("Invalid member for MatchRule: {}", e);
                    return;
                }
            };
        }
    }
    if !path.is_empty() && path != "/" {
        if let Ok(p) = zbus::zvariant::ObjectPath::try_from(path.as_str()) {
            builder = match builder.path(p) {
                Ok(b) => b,
                Err(e) => {
                    warn!("Invalid path for MatchRule: {}", e);
                    return;
                }
            };
        }
    }
    if !service.is_empty() {
        if let Ok(s) = zbus::names::BusName::try_from(service.as_str()) {
            builder = match builder.sender(s) {
                Ok(b) => b,
                Err(e) => {
                    warn!("Invalid sender for MatchRule: {}", e);
                    return;
                }
            };
        }
    }

    let rule = builder.build();
    if let Ok(mut stream) = zbus::MessageStream::for_match_rule(rule, &conn, None).await {
        while let Some(Ok(msg)) = stream.next().await {
            let header = msg.header();
            let iface_str = header
                .interface()
                .map(|i| i.as_str())
                .unwrap_or(&iface)
                .to_string();
            let member_str = header
                .member()
                .map(|m| m.as_str())
                .unwrap_or(&member)
                .to_string();
            let path_str = header
                .path()
                .map(|path| path.as_str())
                .unwrap_or_default()
                .to_string();

            let body_json = match msg.body().deserialize::<zbus::zvariant::OwnedValue>() {
                Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "{}".to_string()),
                Err(_) => "{}".to_string(),
            };

            if signal_tx
                .send((path_str, iface_str, member_str, body_json))
                .await
                .is_err()
            {
                break;
            }
        }
    }
}

#[cfg(test)]
mod generic_dbus_tests {
    use super::*;
    use serde_json::json;

    fn invoke_host_wat(wat_source: &str, manifest: ModuleManifest, export: &str) -> i32 {
        let engine = crate::modules::wasm::WasmEngine::new().unwrap();
        let wasm = wat::parse_str(wat_source).unwrap();
        let module = wasmtime::Module::new(engine.engine(), &wasm).unwrap();
        let mut linker = Linker::new(engine.engine());
        register_host_functions(&mut linker).unwrap();
        let mut state = empty_host_state_for_test();
        state.manifest = manifest;
        let mut store = wasmtime::Store::new(engine.engine(), state);
        store.set_fuel(1_000_000_000).unwrap();
        store.set_epoch_deadline(1_000_000);
        let instance = linker.instantiate(&mut store, &module).unwrap();
        instance
            .get_typed_func::<(), i32>(&mut store, export)
            .unwrap()
            .call(&mut store, ())
            .unwrap()
    }

    #[test]
    fn plain_socket_name_rejects_paths_and_control_characters() {
        assert!(is_plain_file_name("wyrd-audio.sock"));
        for name in [
            "",
            ".",
            "..",
            "../wyrd-audio.sock",
            "nested/wyrd-audio.sock",
            "/tmp/wyrd-audio.sock",
            "wyrd-audio\n.sock",
            "wyrd-audio\0.sock",
        ] {
            assert!(!is_plain_file_name(name), "accepted invalid name {name:?}");
        }
    }

    #[test]
    fn unix_socket_request_rejects_missing_xdg_runtime_dir() {
        assert_eq!(
            resolve_unix_socket_path(std::path::Path::new("wyrd-test.sock"), None),
            Err(-1)
        );
        assert_eq!(
            resolve_unix_socket_path(
                std::path::Path::new("wyrd-test.sock"),
                Some(std::path::PathBuf::from("relative-runtime")),
            ),
            Err(-1)
        );
    }

    #[test]
    fn process_host_rejects_paths_before_resolving_binary() {
        let manifest = ModuleManifest {
            capabilities: vec!["process:spawn:echo".to_string()],
            ..ModuleManifest::default()
        };
        let result = invoke_host_wat(
            r#"(module
                    (import "wyrd_host" "host_exec_process"
                        (func $exec (param i32 i32 i32 i32 i32 i32) (result i32)))
                    (memory (export "memory") 1)
                    (data (i32.const 0) "/tmp/evil/echo")
                    (data (i32.const 32) "[]")
                    (func (export "attempt") (result i32)
                        (call $exec (i32.const 0) (i32.const 14)
                            (i32.const 32) (i32.const 2) (i32.const 64) (i32.const 0)))
                )"#,
            manifest,
            "attempt",
        );
        assert_eq!(result, -403);
    }

    #[test]
    fn image_host_rejects_remote_urls_without_network_capability() {
        let url = "http://127.0.0.1:1/x.png";
        let wat_source = format!(
            r#"(module
                    (import "wyrd_host" "host_image_cached"
                        (func $image (param i32 i32) (result i32)))
                    (memory (export "memory") 1)
                    (data (i32.const 0) "{url}")
                    (func (export "attempt") (result i32)
                        (call $image (i32.const 0) (i32.const {})))
                )"#,
            url.len()
        );
        let manifest = ModuleManifest {
            capabilities: vec!["fs:read:*".to_string()],
            ..ModuleManifest::default()
        };
        assert_eq!(invoke_host_wat(&wat_source, manifest, "attempt"), -403);
    }

    #[test]
    fn write_host_is_disabled_even_for_directly_constructed_capabilities() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("must-not-exist.txt");
        let path = path.to_string_lossy();
        let wat_source = format!(
            r#"(module
                    (import "wyrd_host" "host_write_file"
                        (func $write (param i32 i32 i32 i32) (result i32)))
                    (memory (export "memory") 1)
                    (data (i32.const 0) "{path}")
                    (data (i32.const 256) "payload")
                    (func (export "attempt") (result i32)
                        (call $write (i32.const 0) (i32.const {})
                            (i32.const 256) (i32.const 7)))
                )"#,
            path.len()
        );
        let manifest = ModuleManifest {
            capabilities: vec![format!("fs:write:{path}")],
            ..ModuleManifest::default()
        };
        assert_eq!(invoke_host_wat(&wat_source, manifest, "attempt"), -403);
        assert!(!std::path::Path::new(path.as_ref()).exists());
    }

    #[test]
    fn host_read_file_rejects_devices_and_oversized_files() {
        let device_manifest = ModuleManifest {
            capabilities: vec!["fs:read:/dev/zero".to_string()],
            ..ModuleManifest::default()
        };
        let device_wat = r#"(module
                (import "wyrd_host" "host_read_file"
                    (func $read (param i32 i32 i32 i32) (result i32)))
                (memory (export "memory") 1)
                (data (i32.const 0) "/dev/zero")
                (func (export "attempt") (result i32)
                    (call $read (i32.const 0) (i32.const 9) (i32.const 64) (i32.const 64)))
            )"#;
        assert_eq!(
            invoke_host_wat(device_wat, device_manifest, "attempt"),
            -403
        );

        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("large.txt");
        std::fs::write(&path, vec![b'x'; MAX_READ_FILE_BYTES + 1]).unwrap();
        let path = path.to_string_lossy();
        let large_wat = format!(
            r#"(module
                    (import "wyrd_host" "host_read_file"
                        (func $read (param i32 i32 i32 i32) (result i32)))
                    (memory (export "memory") 1)
                    (data (i32.const 0) "{path}")
                    (func (export "attempt") (result i32)
                        (call $read (i32.const 0) (i32.const {}) (i32.const 64) (i32.const 64)))
                )"#,
            path.len()
        );
        let manifest = ModuleManifest {
            capabilities: vec![format!("fs:read:{path}")],
            ..ModuleManifest::default()
        };
        assert_eq!(invoke_host_wat(&large_wat, manifest, "attempt"), -6);
    }

    fn empty_host_state_for_test() -> ModuleHostState {
        let (updates_tx, _) = mpsc::channel::<(String, ModuleMessage)>(1);
        let (dbus_signal_tx, _) = mpsc::channel(1);
        let (socket_event_tx, _) = mpsc::channel(1);
        let (timer_event_tx, _) = mpsc::channel(1);
        let (path_event_tx, _) = mpsc::channel(1);

        ModuleHostState {
            name: "socket_drop_test".to_string(),
            manifest: ModuleManifest::default(),
            updates_tx,
            dbus_signal_tx,
            socket_event_tx,
            timer_event_tx,
            path_event_tx,
            subscribed_signals: std::collections::HashSet::new(),
            last_updates: std::collections::HashMap::new(),
            active_timers: std::collections::HashSet::new(),
            active_watches: std::collections::HashSet::new(),
            active_sockets: HashMap::new(),
            limits: wasmtime::StoreLimitsBuilder::new()
                .memory_size(64 << 20)
                .instances(1)
                .tables(1)
                .build(),
        }
    }

    #[test]
    fn host_read_dir_returns_too_large_instead_of_truncated_json() {
        let temp_dir = tempfile::tempdir().unwrap();
        for index in 0..10_000 {
            std::fs::write(temp_dir.path().join(format!("entry_{index}")), []).unwrap();
        }
        let path = temp_dir.path().to_string_lossy();
        let wat_source = format!(
            r#"(module
                (import "wyrd_host" "host_read_dir"
                    (func $read_dir (param i32 i32 i32 i32) (result i32)))
                (memory (export "memory") 1)
                (data (i32.const 0) "{path}")
                (func (export "attempt") (result i32)
                    (call $read_dir (i32.const 0) (i32.const {})
                        (i32.const 64) (i32.const 1024)))
            )"#,
            path.len()
        );
        let manifest = ModuleManifest {
            capabilities: vec![format!("fs:read:{path}")],
            ..ModuleManifest::default()
        };
        assert_eq!(invoke_host_wat(&wat_source, manifest, "attempt"), -6);
    }

    #[test]
    fn bounded_command_times_out() {
        let error =
            run_command_with_timeout(std::path::Path::new("/bin/sleep"), &["10".to_string()])
                .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    #[test]
    fn nonblocking_unix_socket_connect_succeeds_for_listener() {
        let temp_dir = tempfile::tempdir().unwrap();
        let socket_path = temp_dir.path().join("host.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        let server = std::thread::spawn(move || {
            let _ = listener.accept().unwrap();
        });

        let stream = connect_unix_socket_with_timeout(&socket_path).unwrap();
        drop(stream);
        server.join().unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn child_reaper_dispatches_later_children_without_waiting_for_first() {
        let (tx, rx) = std::sync::mpsc::channel();
        dispatch_child_reaper(rx);

        let first = std::process::Command::new("/bin/sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let first_pid = first.id() as i32;
        let second = std::process::Command::new("/bin/true").spawn().unwrap();
        let second_pid = second.id();

        tx.send(first).unwrap();
        tx.send(second).unwrap();
        let second_proc = std::path::PathBuf::from(format!("/proc/{second_pid}"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while second_proc.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // SAFETY: this test owns the first child process and uses its PID to ensure cleanup.
        unsafe {
            libc::kill(first_pid, libc::SIGKILL);
        }
        assert!(
            !second_proc.exists(),
            "the later short-lived child remained unreaped behind the long-lived child"
        );
    }

    #[tokio::test]
    async fn module_host_state_drop_closes_active_socket_subscriptions() {
        use std::os::unix::net::UnixStream;

        let (_peer, socket) = UnixStream::pair().unwrap();
        let state_socket = socket.try_clone().unwrap();
        let registry_socket = socket.try_clone().unwrap();
        let handle = NEXT_SUBSCRIPTION_HANDLE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let is_shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader_shutdown = is_shutdown.clone();
        let (socket_event_tx, mut socket_event_rx) = mpsc::channel(1);
        lock_unpoisoned(&ACTIVE_SUBSCRIPTIONS)
            .insert(handle, (is_shutdown.clone(), Some(registry_socket)));

        let mut state = empty_host_state_for_test();
        state.socket_event_tx = socket_event_tx.clone();
        state.active_sockets.insert(handle, state_socket);
        let reader_thread = std::thread::spawn(move || {
            run_socket_reader(handle, socket, socket_event_tx, reader_shutdown)
        });
        drop(state);

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), socket_event_rx.recv())
            .await
            .expect("socket reader should stop after module teardown")
            .expect("socket reader should send its shutdown event");
        assert_eq!(
            event,
            SocketEvent::Closed {
                handle,
                reason: CloseReason::Shutdown
            }
        );
        reader_thread.join().unwrap();
        assert!(is_shutdown.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!lock_unpoisoned(&ACTIVE_SUBSCRIPTIONS).contains_key(&handle));
    }

    #[test]
    fn test_parse_introspect_xml_extracts_methods_and_in_args() {
        let xml = r#"
        <!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN"
        "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
        <node>
          <interface name="org.freedesktop.DBus.Properties">
            <method name="Get">
              <arg type="s" name="interface_name" direction="in"/>
              <arg type="s" name="property_name" direction="in"/>
              <arg type="v" name="value" direction="out"/>
            </method>
          </interface>
          <interface name="org.example.CustomService">
            <method name="ComplexCall">
              <arg type="a{sa{sv}}" name="settings" direction="in"/>
              <arg type="o" name="target_device" direction="in"/>
              <arg type="b" name="enable" direction="in"/>
              <arg type="o" name="result_path" direction="out"/>
            </method>
            <method name="Ping"/>
          </interface>
        </node>
        "#;

        let res = parse_introspect_xml(xml, "org.example.CustomService", "ComplexCall");
        assert_eq!(
            res,
            Some((
                "org.example.CustomService".to_string(),
                vec!["a{sa{sv}}".to_string(), "o".to_string(), "b".to_string()]
            ))
        );

        let res_infer = parse_introspect_xml(xml, "", "ComplexCall");
        assert_eq!(
            res_infer,
            Some((
                "org.example.CustomService".to_string(),
                vec!["a{sa{sv}}".to_string(), "o".to_string(), "b".to_string()]
            ))
        );

        let res_ping = parse_introspect_xml(xml, "org.example.CustomService", "Ping");
        assert_eq!(
            res_ping,
            Some(("org.example.CustomService".to_string(), vec![]))
        );
    }

    #[test]
    fn test_json_to_dbus_arg_signature_coercion() {
        // ObjectPath coercion
        let op_arg = json_to_dbus_arg(&json!("/org/custom/path"), Some("o"));
        assert_eq!(op_arg.signature().to_string(), "o");

        // Boolean coercion
        let bool_arg = json_to_dbus_arg(&json!(true), Some("b"));
        assert_eq!(bool_arg.signature().to_string(), "b");

        // Nested dictionary a{sa{sv}}
        let settings_json = json!({
            "connection": { "id": "MyWifi", "type": "802-11-wireless" },
            "802-11-wireless": { "ssid": [119, 105, 102, 105] }
        });
        let dict_arg = json_to_dbus_arg(&settings_json, Some("a{sa{sv}}"));
        assert_eq!(dict_arg.signature().to_string(), "a{sa{sv}}");

        // Dictionary a{sv}
        let dict_sv = json_to_dbus_arg(&json!({ "flag": true }), Some("a{sv}"));
        assert_eq!(dict_sv.signature().to_string(), "a{sv}");

        // Array of strings as
        let arr_str = json_to_dbus_arg(&json!(["a", "b"]), Some("as"));
        assert_eq!(arr_str.signature().to_string(), "as");

        // Array of bytes ay
        let arr_bytes = json_to_dbus_arg(&json!([1, 2, 3]), Some("ay"));
        assert_eq!(arr_bytes.signature().to_string(), "ay");
    }

    #[test]
    fn test_deserialize_dbus_reply_structure() {
        let op1 = zbus::zvariant::ObjectPath::try_from("/path/1").unwrap();
        let op2 = zbus::zvariant::ObjectPath::try_from("/path/2").unwrap();
        let call = zbus::message::Message::method_call("/", "foo")
            .unwrap()
            .build(&())
            .unwrap();
        let msg = zbus::message::Message::method_return(&call.header())
            .unwrap()
            .build(&(op1, op2))
            .unwrap();

        let json_str = deserialize_dbus_reply_to_json(&msg).unwrap();
        let json_val: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(json_val, json!(["/path/1", "/path/2"]));
    }

    #[test]
    fn test_deserialize_dbus_reply_managed_objects() {
        use std::collections::HashMap;
        let op = zbus::zvariant::OwnedObjectPath::try_from("/org/example/dev0").unwrap();
        let mut props = HashMap::new();
        let name_val: zbus::zvariant::OwnedValue =
            zbus::zvariant::Value::from("Device0").try_into().unwrap();
        props.insert("Name".to_string(), name_val);
        props.insert(
            "Powered".to_string(),
            zbus::zvariant::OwnedValue::from(true),
        );

        let mut ifaces = HashMap::new();
        ifaces.insert("org.example.Device1".to_string(), props);

        let mut root = HashMap::new();
        root.insert(op, ifaces);

        let call = zbus::message::Message::method_call("/", "GetManagedObjects")
            .unwrap()
            .build(&())
            .unwrap();
        let msg = zbus::message::Message::method_return(&call.header())
            .unwrap()
            .build(&(root,))
            .unwrap();

        let json_str = deserialize_dbus_reply_to_json(&msg).unwrap();
        let json_val: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(
            json_val,
            json!({
                "/org/example/dev0": {
                    "org.example.Device1": {
                        "Name": "Device0",
                        "Powered": true
                    }
                }
            })
        );
    }

    #[tokio::test]
    async fn test_socket_reader_eof_delivers_closed_peer_closed() {
        use std::io::Write;
        use std::os::unix::net::UnixStream;
        use tokio::sync::mpsc;

        let (mut client, server) = UnixStream::pair().unwrap();
        let (tx, mut rx) = mpsc::channel::<SocketEvent>(16);
        let is_shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        let reader_handle = std::thread::spawn(move || {
            run_socket_reader(101, server, tx, is_shutdown);
        });

        client.write_all(b"{\"event\":\"update\"}\n").unwrap();
        client.flush().unwrap();

        let event1 = rx.recv().await.unwrap();
        match event1 {
            SocketEvent::Data { handle, payload } => {
                assert_eq!(handle, 101);
                assert_eq!(payload, b"{\"event\":\"update\"}");
            }
            other => panic!("expected Data event, got {:?}", other),
        }

        // Close client -> peer closed EOF
        drop(client);
        let event2 = rx.recv().await.unwrap();
        match event2 {
            SocketEvent::Closed { handle, reason } => {
                assert_eq!(handle, 101);
                assert_eq!(reason, CloseReason::PeerClosed);
            }
            other => panic!("expected Closed event, got {:?}", other),
        }

        reader_handle.join().unwrap();
    }

    #[tokio::test]
    async fn test_socket_reader_payload_limit_enforced() {
        use std::io::Write;
        use std::os::unix::net::UnixStream;
        use tokio::sync::mpsc;

        let (mut client, server) = UnixStream::pair().unwrap();
        let (tx, mut rx) = mpsc::channel::<SocketEvent>(16);
        let is_shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        let reader_handle = std::thread::spawn(move || {
            run_socket_reader(102, server, tx, is_shutdown);
        });

        // Send 2 MiB in chunks without newline -> exceeds MAX_SOCKET_PAYLOAD_BYTES (1 MiB)
        std::thread::spawn(move || {
            let chunk = vec![b'A'; 64 * 1024];
            for _ in 0..32 {
                if client.write_all(&chunk).is_err() {
                    break;
                }
            }
        });

        let event1 = rx.recv().await.unwrap();
        assert!(matches!(
            event1,
            SocketEvent::Error {
                handle: 102,
                code: -1
            }
        ));

        let event2 = rx.recv().await.unwrap();
        match event2 {
            SocketEvent::Closed { handle, reason } => {
                assert_eq!(handle, 102);
                assert_eq!(
                    reason,
                    CloseReason::ReadError("payload too large".to_string())
                );
            }
            other => panic!("expected Closed event, got {:?}", other),
        }

        reader_handle.join().unwrap();
    }
}
