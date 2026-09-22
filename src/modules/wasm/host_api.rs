//! Host API functions provided to WASM guest modules.

use super::guest_abi::GuestExports;
use crate::modules::manifest::ModuleManifest;
use crate::modules::ModuleMessage;
use anyhow::Result;
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use tokio::sync::mpsc;
use wasmtime::{Caller, Linker};
use zbus::zvariant::{DynamicTuple, DynamicType, OwnedObjectPath, Signature, Type, Value};

pub struct ModuleHostState {
    pub name: String,
    pub manifest: ModuleManifest,
    pub updates_tx: mpsc::Sender<(String, ModuleMessage)>,
    pub dbus_signal_tx: mpsc::Sender<(String, String, String)>,
    pub socket_event_tx: mpsc::Sender<String>,
}

fn resolve_binary_in_path(prog_name: &str) -> Option<std::path::PathBuf> {
    if prog_name.is_empty() {
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
    // 1. host_send_update(widget_ptr, widget_len, payload_ptr, payload_len) -> u32
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

    // 2. host_request_surface(action_ptr, action_len, params_ptr, params_len) -> u32
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

    // 3. host_publish(topic_ptr, topic_len, value_ptr, value_len) -> u32
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

    // 4. host_request_capability(cap_ptr, cap_len, action_ptr, action_len, params_ptr, params_len) -> u32
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

    // 5. host_focus_request(popup_ptr, popup_len) -> u32
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

    // 6. host_log(level: u32, msg_ptr: u32, msg_len: u32)
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

    // 7. host_now_ms() -> u64
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

    // 7b. host_local_offset_seconds() -> i32
    linker.func_wrap(
        "wyrd_host",
        "host_local_offset_seconds",
        |_caller: Caller<'_, ModuleHostState>| -> i32 {
            // SAFETY: Calling standard POSIX `time` and reentrant `localtime_r`.
            // `tm` is properly initialized by `localtime_r` before being assumed initialized.
            unsafe {
                let now = libc::time(std::ptr::null_mut());
                let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
                libc::localtime_r(&now, tm.as_mut_ptr());
                let tm = tm.assume_init();
                tm.tm_gmtoff as i32
            }
        },
    )?;

    // 8. host_read_file(path_ptr, path_len, out_ptr, out_max_len) -> i32
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

            let path_obj = std::path::Path::new(&path_str);
            let effective_path = if path_obj.is_relative() {
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
            } else {
                path_obj.to_path_buf()
            };

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

            match std::fs::read_to_string(&effective_path) {
                Ok(content) => {
                    let bytes = content.as_bytes();
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
                Err(_) => -4,
            }
        },
    )?;

    // 8b. host_read_dir(path_ptr, path_len, out_ptr, out_max_len) -> i32
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

            if !caller.data().manifest.can_read_path(&path_str) {
                warn!(
                    "Module '{}' denied reading directory '{}' (capability missing)",
                    caller.data().name,
                    path_str
                );
                return -403;
            }

            let entries = match std::fs::read_dir(&path_str) {
                Ok(rd) => {
                    let names: Vec<String> = rd
                        .flatten()
                        .filter_map(|e| e.file_name().into_string().ok())
                        .collect();
                    serde_json::to_string(&names).unwrap_or_else(|_| "[]".to_string())
                }
                Err(_) => "[]".to_string(),
            };

            let bytes = entries.as_bytes();
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

    // 9. host_exec_process(prog_ptr, prog_len, args_ptr, args_len, out_ptr, out_max_len) -> i32
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

            if !caller.data().manifest.can_spawn_binary(prog_name) {
                warn!(
                    "Module '{}' denied executing program '{}' (missing process:spawn:{} capability)",
                    caller.data().name,
                    prog,
                    prog_name
                );
                return -403;
            }

            let resolved_path = match resolve_binary_in_path(prog_name) {
                Some(p) => p,
                None => {
                    warn!("host_exec_process binary '{}' not found in $PATH", prog_name);
                    return -4;
                }
            };

            if out_max_len == 0 {
                return match std::process::Command::new(&resolved_path)
                    .args(&args)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok(_) => 0,
                    Err(e) => {
                        warn!("host_exec_process failed to spawn '{}': {}", prog_name, e);
                        -4
                    }
                };
            }

            let output = run_blocking(|| {
                std::process::Command::new(&resolved_path)
                    .args(&args)
                    .output()
            });

            match output {
                Ok(out) => {
                    if !out.status.success() {
                        warn!(
                            "host_exec_process '{}' exited with non-zero status: {:?}",
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
                    warn!("host_exec_process failed to execute '{}': {}", prog_name, e);
                    -4
                }
            }
        },
    )?;

    // 10. host_exec_command(cmd_ptr, cmd_len, out_ptr, out_max_len) -> i32 (Backward compatibility)
    // NOTE: This tokenizes cmd_str and executes via Command::new(prog).args(args) directly, NEVER sh -c!
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

            if !caller.data().manifest.can_spawn_binary(prog_name) {
                warn!(
                    "Module '{}' denied executing command '{}' (missing process:spawn:{} capability)",
                    caller.data().name,
                    cmd_str,
                    prog_name
                );
                return -403;
            }

            let resolved_path = match resolve_binary_in_path(prog_name) {
                Some(p) => p,
                None => {
                    warn!("host_exec_command binary '{}' not found in $PATH", prog_name);
                    return -4;
                }
            };

            let output = run_blocking(|| {
                std::process::Command::new(&resolved_path)
                    .args(args)
                    .output()
            });

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
                    -4
                }
            }
        },
    )?;

    // 11. host_dbus_call(bus_ptr, bus_len, service_ptr, service_len, path_ptr, path_len, iface_ptr, iface_len, member_ptr, member_len, args_ptr, args_len, out_ptr, out_max_len) -> i32
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

    // 12. host_dbus_subscribe(bus_ptr, bus_len, service_ptr, service_len, path_ptr, path_len, iface_ptr, iface_len, member_ptr, member_len) -> i32
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

            let signal_tx = caller.data().dbus_signal_tx.clone();
            tokio::spawn(async move {
                subscribe_dbus_signal(bus, service, path, iface, member, signal_tx).await;
            });

            0
        },
    )?;

    // 19. host_file_exists(path_ptr, path_len) -> i32
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
                return -1; // Sentinel for permission denied / failure
            }

            if std::path::Path::new(&path_str).exists() {
                1
            } else {
                0
            }
        },
    )?;

    // 20. host_list_dir(path_ptr, path_len, out_ptr, out_max_len) -> i32
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

    // 21. host_unix_socket_request(path_ptr, path_len, payload_ptr, payload_len, out_ptr, out_max_len) -> i32
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

            let payload_str =
                match GuestExports::read_string(&memory, &caller, payload_ptr, payload_len) {
                    Ok(s) => s,
                    Err(_) => return -2,
                };

            let path_obj = std::path::Path::new(&path_str);
            let effective_path = if path_obj.is_relative() {
                let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
                runtime_dir.join(path_obj)
            } else {
                path_obj.to_path_buf()
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
                use std::os::unix::net::UnixStream;
                use std::time::Duration;

                let mut stream = UnixStream::connect(&effective_path).map_err(|e| {
                    debug!(
                        "host_unix_socket_request connect to {:?} failed: {}",
                        effective_path, e
                    );
                    -404
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

                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).map_err(|e| {
                    warn!("host_unix_socket_request read_line failed: {}", e);
                    -4
                })?;

                Ok(line)
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
            let path = std::env::var_os("XDG_RUNTIME_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                .join(&socket_name);
            if !caller.data().manifest.can_unix_socket_connect(&socket_name) {
                return -403;
            }
            let event_tx = caller.data().socket_event_tx.clone();
            std::thread::spawn(move || {
                use std::io::{BufRead, BufReader, Write};
                use std::os::unix::net::UnixStream;
                loop {
                    let Ok(mut stream) = UnixStream::connect(&path) else {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        continue;
                    };
                    if stream.write_all(b"{\"cmd\":\"subscribe\"}\n").is_err() {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        continue;
                    }
                    let reader = BufReader::new(stream);
                    let mut disconnected = false;
                    for line in reader.lines() {
                        let Ok(line) = line else {
                            disconnected = true;
                            break;
                        };
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                            if let Some(event) = value.get("event").and_then(|v| v.as_str()) {
                                if event_tx.blocking_send(event.to_string()).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    if disconnected {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                }
            });
            0
        },
    )?;

    // 25. host_get_env(key_ptr, key_len, out_ptr, out_max_len) -> i32
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
type MethodCacheVal = (String, Vec<String>);

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
        if let Some(val) = guard.get(&key) {
            return val.clone();
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
                guard.insert(key, (discovered_iface.clone(), in_args.clone()));
            }
            return (discovered_iface, in_args);
        }
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
            if let Some(arr) = v.as_array() {
                if !arr.is_empty() && arr.iter().all(|x| x.as_u64().is_some_and(|n| n <= 255)) {
                    let bytes: Vec<u8> = arr
                        .iter()
                        .filter_map(|x| x.as_u64().map(|n| n as u8))
                        .collect();
                    map.insert(k.clone(), Value::from(bytes));
                    continue;
                }
            }
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

    // Fast path for Properties.Get and Properties.GetAll
    if (iface == "org.freedesktop.DBus.Properties" || iface.is_empty()) && member == "Get" {
        if let serde_json::Value::Array(ref arr) = args_val {
            if arr.len() >= 2 {
                let target_iface = arr[0].as_str().unwrap_or("");
                let prop_name = arr[1].as_str().unwrap_or("");
                let proxy = zbus::blocking::Proxy::new(
                    &conn,
                    service,
                    path,
                    "org.freedesktop.DBus.Properties",
                )?;
                let val: zbus::zvariant::OwnedValue =
                    proxy.call("Get", &(target_iface, prop_name))?;
                let json_val = zvariant_to_json(&val);
                return Ok(serde_json::to_string(&json_val)?);
            }
        }
    } else if (iface == "org.freedesktop.DBus.Properties" || iface.is_empty()) && member == "GetAll"
    {
        if let serde_json::Value::Array(ref arr) = args_val {
            if let Some(target_iface) = arr.first().and_then(|v| v.as_str()) {
                let proxy = zbus::blocking::Proxy::new(
                    &conn,
                    service,
                    path,
                    "org.freedesktop.DBus.Properties",
                )?;
                let map: std::collections::HashMap<String, zbus::zvariant::OwnedValue> =
                    proxy.call("GetAll", &(target_iface,))?;
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
                let proxy = zbus::blocking::Proxy::new(
                    &conn,
                    service,
                    path,
                    "org.freedesktop.DBus.Properties",
                )?;
                let _: () = proxy.call("Set", &(target_iface, prop_name, val))?;
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

async fn subscribe_dbus_signal(
    bus: String,
    service: String,
    path: String,
    iface: String,
    member: String,
    signal_tx: mpsc::Sender<(String, String, String)>,
) {
    use futures_util::StreamExt;
    let conn = match if bus == "system" {
        zbus::Connection::system().await
    } else {
        zbus::Connection::session().await
    } {
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

            let body_json = match msg.body().deserialize::<zbus::zvariant::OwnedValue>() {
                Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "{}".to_string()),
                Err(_) => "{}".to_string(),
            };

            if signal_tx
                .send((iface_str, member_str, body_json))
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

        // 1. Specified interface and method
        let res = parse_introspect_xml(xml, "org.example.CustomService", "ComplexCall");
        assert_eq!(
            res,
            Some((
                "org.example.CustomService".to_string(),
                vec!["a{sa{sv}}".to_string(), "o".to_string(), "b".to_string()]
            ))
        );

        // 2. Interface omitted (inferred)
        let res_infer = parse_introspect_xml(xml, "", "ComplexCall");
        assert_eq!(
            res_infer,
            Some((
                "org.example.CustomService".to_string(),
                vec!["a{sa{sv}}".to_string(), "o".to_string(), "b".to_string()]
            ))
        );

        // 3. Zero-arg method
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
}
