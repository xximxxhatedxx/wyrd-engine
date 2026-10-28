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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, RwLock};

    #[test]
    fn lock_unpoisoned_recovers_after_panic() {
        let mutex = Arc::new(Mutex::new(42));
        let m = Arc::clone(&mutex);
        let _ = std::thread::spawn(move || {
            let _guard = m.lock().unwrap();
            panic!("simulated worker panic");
        })
        .join();

        // Mutex is now poisoned. lock_unpoisoned must recover.
        let mut guard = super::lock_unpoisoned(&mutex);
        assert_eq!(*guard, 42);
        *guard = 100;
        drop(guard);
        assert_eq!(*super::lock_unpoisoned(&mutex), 100);
    }

    #[test]
    fn read_unpoisoned_recovers_after_panic() {
        let lock = Arc::new(RwLock::new(42));
        let l = Arc::clone(&lock);
        let _ = std::thread::spawn(move || {
            let _guard = l.write().unwrap();
            panic!("simulated worker write panic");
        })
        .join();

        // RwLock is now poisoned. read_unpoisoned must recover.
        let guard = super::read_unpoisoned(&lock);
        assert_eq!(*guard, 42);
    }

    #[test]
    fn write_unpoisoned_recovers_after_panic() {
        let lock = Arc::new(RwLock::new(42));
        let l = Arc::clone(&lock);
        let _ = std::thread::spawn(move || {
            let _guard = l.write().unwrap();
            panic!("simulated worker write panic");
        })
        .join();

        // RwLock is now poisoned. write_unpoisoned must recover.
        let mut guard = super::write_unpoisoned(&lock);
        assert_eq!(*guard, 42);
        *guard = 200;
        drop(guard);
        assert_eq!(*super::read_unpoisoned(&lock), 200);
    }
}
