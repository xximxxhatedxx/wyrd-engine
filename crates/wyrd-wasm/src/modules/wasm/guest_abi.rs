//! WASM Guest ABI & Memory Management.

use anyhow::{bail, Context, Result};
use wasmtime::{Instance, Memory, Store, TypedFunc};

pub type DbusSignalParams = (u32, u32, u32, u32, u32, u32);

pub struct GuestExports {
    pub memory: Memory,
    pub alloc: Option<TypedFunc<u32, u32>>,
    pub dealloc: Option<TypedFunc<(u32, u32), ()>>,
    pub init: Option<TypedFunc<(u32, u32), u32>>,
    pub on_event: Option<TypedFunc<(u32, u32, u32, u32), ()>>,
    pub on_topic: Option<TypedFunc<(u32, u32, u32, u32), ()>>,
    pub on_tick: Option<TypedFunc<(), ()>>,
    pub on_dbus_signal: Option<TypedFunc<DbusSignalParams, ()>>,
    pub shutdown: Option<TypedFunc<(), ()>>,
}

impl GuestExports {
    pub fn extract<T>(store: &mut Store<T>, instance: &Instance) -> Result<Self> {
        let memory = instance
            .get_memory(&mut *store, "memory")
            .context("WASM module missing exported 'memory'")?;

        let alloc = instance
            .get_typed_func::<u32, u32>(&mut *store, "wyrd_alloc")
            .or_else(|_| instance.get_typed_func::<u32, u32>(&mut *store, "alloc"))
            .ok();

        let dealloc = instance
            .get_typed_func::<(u32, u32), ()>(&mut *store, "wyrd_dealloc")
            .or_else(|_| instance.get_typed_func::<(u32, u32), ()>(&mut *store, "dealloc"))
            .ok();

        let init = instance
            .get_typed_func::<(u32, u32), u32>(&mut *store, "wyrd_init")
            .or_else(|_| instance.get_typed_func::<(u32, u32), u32>(&mut *store, "init"))
            .ok();

        let on_event = instance
            .get_typed_func::<(u32, u32, u32, u32), ()>(&mut *store, "wyrd_on_event")
            .or_else(|_| {
                instance.get_typed_func::<(u32, u32, u32, u32), ()>(&mut *store, "on_event")
            })
            .ok();

        let on_topic = instance
            .get_typed_func::<(u32, u32, u32, u32), ()>(&mut *store, "wyrd_on_topic")
            .or_else(|_| {
                instance.get_typed_func::<(u32, u32, u32, u32), ()>(&mut *store, "on_topic")
            })
            .ok();

        let on_tick = instance
            .get_typed_func::<(), ()>(&mut *store, "wyrd_on_tick")
            .or_else(|_| instance.get_typed_func::<(), ()>(&mut *store, "on_tick"))
            .or_else(|_| instance.get_typed_func::<(), ()>(&mut *store, "tick"))
            .ok();

        let on_dbus_signal = instance
            .get_typed_func::<(u32, u32, u32, u32, u32, u32), ()>(
                &mut *store,
                "wyrd_on_dbus_signal",
            )
            .or_else(|_| {
                instance.get_typed_func::<(u32, u32, u32, u32, u32, u32), ()>(
                    &mut *store,
                    "on_dbus_signal",
                )
            })
            .ok();

        let shutdown = instance
            .get_typed_func::<(), ()>(&mut *store, "wyrd_shutdown")
            .or_else(|_| instance.get_typed_func::<(), ()>(&mut *store, "shutdown"))
            .ok();

        Ok(Self {
            memory,
            alloc,
            dealloc,
            init,
            on_event,
            on_topic,
            on_tick,
            on_dbus_signal,
            shutdown,
        })
    }

    pub fn write_bytes<T>(&self, store: &mut Store<T>, data: &[u8]) -> Result<(u32, u32)> {
        let len = data.len() as u32;
        if len == 0 {
            return Ok((0, 0));
        }

        let ptr = if let Some(alloc) = &self.alloc {
            alloc.call(&mut *store, len)?
        } else {
            bail!("WASM module cannot receive data: missing 'wyrd_alloc' export");
        };

        let mem_data = self.memory.data_mut(&mut *store);
        let start = ptr as usize;
        let end = start + len as usize;
        if end > mem_data.len() {
            bail!(
                "WASM module memory buffer out of bounds (len={}, mem_size={})",
                end,
                mem_data.len()
            );
        }

        mem_data[start..end].copy_from_slice(data);
        Ok((ptr, len))
    }

    pub fn free_bytes<T>(&self, store: &mut Store<T>, ptr: u32, len: u32) -> Result<()> {
        if ptr != 0 && len != 0 {
            if let Some(dealloc) = &self.dealloc {
                dealloc.call(&mut *store, (ptr, len))?;
            }
        }
        Ok(())
    }

    pub fn read_string<T, S: wasmtime::AsContext<Data = T>>(
        memory: &Memory,
        store: S,
        ptr: u32,
        len: u32,
    ) -> Result<String> {
        let mem_data = memory.data(store.as_context());
        let start = ptr as usize;
        let end = start + len as usize;
        if end > mem_data.len() {
            bail!(
                "read_string out of bounds: range [{}..{}] exceeds memory size {}",
                start,
                end,
                mem_data.len()
            );
        }
        let s =
            std::str::from_utf8(&mem_data[start..end]).context("invalid UTF-8 in guest memory")?;
        Ok(s.to_string())
    }
}
