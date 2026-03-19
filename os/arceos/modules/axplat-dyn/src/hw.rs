//! Hardware-related methods

use axplat::hw::HwIf;

struct HwImpl;

#[impl_plat_interface]
impl HwIf for HwImpl {
    /// Get the number of CPU cores available on this platform.
    fn cpu_num() -> usize {
        somehal::smp::cpu_meta_list().count()
    }

    fn early_percpu_area() -> Option<*mut u8> {
        Some(somehal::smp::percpu_data_ptr(0).unwrap_or_default())
    }
}
