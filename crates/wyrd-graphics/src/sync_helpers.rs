//! Poison-resilient synchronization helpers.
//!
//! A panic in a worker or background thread should not cascade into crashing the
//! main compositor shell or daemon when subsequent threads acquire a [`Mutex`] or [`RwLock`].

use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Acquires a [`Mutex`] guard, recovering the inner value if a previous lock holder panicked.
#[inline]
pub fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Acquires a shared [`RwLock`] read guard, recovering the inner value if poisoned.
#[inline]
pub fn read_unpoisoned<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Acquires an exclusive [`RwLock`] write guard, recovering the inner value if poisoned.
#[inline]
pub fn write_unpoisoned<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
