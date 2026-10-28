/// Requested rendering execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RenderMode {
    /// Selects GPU if supported and available, otherwise falls back to CPU.
    Auto,
    /// Explicitly requests hardware-accelerated GPU rendering.
    Gpu,
    /// Explicitly requests software rasterization via CPU.
    Cpu,
}

/// Detected rendering capabilities of the host system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderCapabilities {
    /// Whether a compatible GPU adapter is detected and usable.
    pub gpu_available: bool,
}

impl RenderCapabilities {
    /// Probes the system for GPU rendering support.
    pub fn detect() -> Self {
        Self {
            gpu_available: gpu_available(),
        }
    }

    /// Resolves the actual [`RenderMode`] to use given the requested mode and system capabilities.
    pub fn select(self, requested: RenderMode) -> RenderMode {
        match requested {
            RenderMode::Cpu => RenderMode::Cpu,
            RenderMode::Gpu if self.gpu_available => RenderMode::Gpu,
            RenderMode::Gpu => {
                log::warn!("GPU renderer requested but no compatible adapter was found; using CPU renderer");
                RenderMode::Cpu
            }
            RenderMode::Auto if self.gpu_available => RenderMode::Gpu,
            RenderMode::Auto => {
                log::info!("No compatible GPU adapter found; using CPU renderer");
                RenderMode::Cpu
            }
        }
    }
}

#[cfg(feature = "gpu-render")]
fn gpu_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let instance = super::gpu::GpuRenderer::create_wgpu_instance();
        !pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())).is_empty()
    })
}

#[cfg(not(feature = "gpu-render"))]
fn gpu_available() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::{RenderCapabilities, RenderMode};

    #[test]
    fn auto_uses_cpu_when_gpu_is_unavailable() {
        let capabilities = RenderCapabilities {
            gpu_available: false,
        };
        assert_eq!(capabilities.select(RenderMode::Auto), RenderMode::Cpu);
    }

    #[test]
    fn select_gpu_without_adapter_falls_back_to_cpu() {
        let capabilities = RenderCapabilities {
            gpu_available: false,
        };
        assert_eq!(capabilities.select(RenderMode::Gpu), RenderMode::Cpu);
    }

    #[test]
    fn explicit_cpu_always_wins() {
        let capabilities = RenderCapabilities {
            gpu_available: true,
        };
        assert_eq!(capabilities.select(RenderMode::Cpu), RenderMode::Cpu);
        assert_eq!(capabilities.select(RenderMode::Auto), RenderMode::Gpu);
    }
}
