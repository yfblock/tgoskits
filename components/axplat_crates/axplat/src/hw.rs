//! Hardware-related methods.

/// Hardware-related methods.
#[def_plat_interface]
pub trait HwIf {
    /// Get the number of CPU cores available on this platform.
    ///
    /// The platform should either get this value statically from its
    /// configuration or dynamically by platform-specific methods.
    ///
    /// For statically configured platforms, by convention, this value should be
    /// the same as `MAX_CPU_NUM` defined in the platform configuration.
    fn cpu_num() -> usize;

    /// Get the early percpu area size.
    /// 
    /// None should be returned if the platform uses link-time-allocated percpu
    /// area.
    fn early_percpu_area() -> Option<*mut u8> {
        None
    }
}