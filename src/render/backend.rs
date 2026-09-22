//! Render backend selection for the shell runtime.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    Auto,
    Gpu,
    Cpu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderCapabilities {
    pub gpu_available: bool,
    pub cpu_fallback: bool,
}

impl RenderCapabilities {
    pub fn detect() -> Self {
        Self {
            gpu_available: gpu_available(),
            cpu_fallback: true,
        }
    }

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
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    !pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())).is_empty()
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
            cpu_fallback: true,
        };
        assert_eq!(capabilities.select(RenderMode::Auto), RenderMode::Cpu);
        assert_eq!(capabilities.select(RenderMode::Gpu), RenderMode::Cpu);
    }

    #[test]
    fn explicit_cpu_always_wins() {
        let capabilities = RenderCapabilities {
            gpu_available: true,
            cpu_fallback: true,
        };
        assert_eq!(capabilities.select(RenderMode::Cpu), RenderMode::Cpu);
        assert_eq!(capabilities.select(RenderMode::Auto), RenderMode::Gpu);
    }
}
