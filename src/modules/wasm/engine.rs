//! WASM Engine setup.

use anyhow::{Context, Result};
use wasmtime::{Config, Engine};

#[derive(Clone)]
pub struct WasmEngine {
    engine: Engine,
}

impl WasmEngine {
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.consume_fuel(true);
        config.cranelift_opt_level(wasmtime::OptLevel::Speed);
        // Optimize memory footprint for embedded sandbox modules:
        // Disable multi-gigabyte static memory reservation per instance.
        // Use dynamic linear memory allocation with minimal guard sizes.
        config.memory_guard_size(0);
        config.memory_reservation(0);
        config.memory_init_cow(true);
        config.parallel_compilation(false);

        let engine = Engine::new(&config).context("failed to initialize Wasmtime Engine")?;
        Ok(Self { engine })
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }
}

impl Default for WasmEngine {
    fn default() -> Self {
        Self::new().expect("default WasmEngine initialization failed")
    }
}
