//! Hardware-related methods.

use axplat::hw::HwIf;

struct HwImpl;

#[impl_plat_interface]
impl HwIf for HwImpl {
    /// Get the number of CPU cores available on this platform.
    fn cpu_num() -> usize {
        crate::config::plat::MAX_CPU_NUM
    }
}