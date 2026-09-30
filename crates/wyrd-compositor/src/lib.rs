//! Wayland compositor integrations (Hyprland, Sway, Niri, and fallback) for Wyrd.

// Keep in sync with wyrd-graphics::sync_helpers
#[allow(dead_code)]
pub(crate) mod sync_helpers {
    use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

    #[inline]
    pub fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[inline]
    pub fn read_unpoisoned<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
        lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[inline]
    pub fn write_unpoisoned<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
        lock.write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub mod compositor;
pub use compositor::*;
