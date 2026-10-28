use std::env;
use std::ffi::CString;
use std::fs;
use std::os::fd::{AsFd, AsRawFd, FromRawFd};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::io::OwnedFd;
use std::path::PathBuf;

use anyhow::Context;
use wayland_client::protocol::wl_shm::Format;
use wyrd_wayland::wayland::backend::WaylandBackend;

fn skip_reason(display: Option<&std::ffi::OsStr>) -> Option<String> {
    if display.is_none_or(|display| display.is_empty()) {
        Some("skipping: WAYLAND_DISPLAY not set".to_string())
    } else {
        None
    }
}

#[tokio::test]
async fn connects_to_active_compositor_and_creates_wl_shm_buffer() -> anyhow::Result<()> {
    let Some(display) = env::var_os("WAYLAND_DISPLAY") else {
        eprintln!("skipping: WAYLAND_DISPLAY not set");
        return Ok(());
    };

    if display.is_empty() {
        eprintln!("skipping: WAYLAND_DISPLAY not set");
        return Ok(());
    }

    let display = display.to_string_lossy();
    let runtime_dir = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/user/1000"));
    let socket = runtime_dir.join(display.as_ref());
    let socket_exists =
        fs::symlink_metadata(&socket).is_ok_and(|metadata| metadata.file_type().is_socket());
    if !socket_exists {
        eprintln!(
            "skipping: compositor socket not found at {socket:?} for WAYLAND_DISPLAY={display}"
        );
        return Ok(());
    }

    println!("connecting to Wayland compositor {display}");

    let (backend, _state) = WaylandBackend::<()>::connect()
        .await
        .with_context(|| format!("failed to connect to compositor {display}"))?;

    if env::var_os("WLR_LIBINPUT_NO_DEVICES").is_some_and(|value| value == "1") {
        let state = wyrd_sync::lock_unpoisoned(&_state);
        let input = wyrd_sync::read_unpoisoned(&state.input_state);
        assert!(
            input.wayland_pointer.is_none(),
            "pointer proxy must not be created without libinput devices"
        );
        assert!(
            input.wayland_keyboard.is_none(),
            "keyboard proxy must not be created without libinput devices"
        );
    }

    let name = CString::new("wyrd_real_compositor_smoke")?;
    // SAFETY: `memfd_create` is invoked with a valid null-terminated C string, and `raw_fd` is verified non-negative.
    let raw_fd = unsafe { libc::memfd_create(name.as_ptr(), 0) };
    if raw_fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: `raw_fd` is a valid descriptor returned by memfd_create.
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    // SAFETY: `ftruncate` operates on an open exclusively-owned anonymous memfd.
    if unsafe { libc::ftruncate(fd.as_raw_fd(), 16 * 16 * 4) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    let pool = backend
        .shm
        .create_pool(fd.as_fd(), 16 * 16 * 4_i32, &backend.qh, ());
    let buffer = pool.create_buffer(0, 16, 16, 64, Format::Xrgb8888, &backend.qh, 0);

    backend.conn.roundtrip()?;
    drop(buffer);
    drop(pool);
    Ok(())
}

#[test]
fn skips_without_wayland_display() {
    assert_eq!(
        skip_reason(None),
        Some("skipping: WAYLAND_DISPLAY not set".to_string())
    );
}
