//! Lua 5.4 scripting runtime, host extension trait, and hot-reload file watcher for Wyrd.
//!
//! This crate depends only on `wyrd-sync` so lightweight consumers
//! (such as `wyrd-wallpaper`) can embed the Lua runtime without pulling in
//! widget layout or Wayland surface layers.

pub mod config;
pub use config::*;
