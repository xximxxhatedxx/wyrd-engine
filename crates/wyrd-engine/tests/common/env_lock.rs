//! # Why this lock exists (and not a force_cpu parameter)
//!
//! RenderContext::with_gpu_mode reads WYRD_FORCE_CPU from the
//! process environment. Tests that verify the env-var path must
//! mutate that variable, which is process-global state. A
//! with_mode_and_force_cpu(scale, mode, bool) overload was
//! considered and rejected because it would bypass the very code
//! path we want to test (the env-var read).
//!
//! Contract: any test that calls std::env::set_var /
//! std::env::remove_var for env vars consumed by the render
//! pipeline MUST hold this lock for the duration of the mutation
//! and any subsequent read.

use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Guard holding the global environment lock.
///
/// Any test reading or mutating process-level environment variables (such as `WYRD_FORCE_CPU`)
/// MUST acquire this lock and hold it for the duration of the test.
pub fn acquire_env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
