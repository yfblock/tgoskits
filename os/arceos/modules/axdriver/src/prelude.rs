//! Device driver prelude that includes some traits and types.

pub use axdriver_base::{BaseDriverOps, DevError, DevResult, DeviceType};
#[cfg(feature = "block")]
pub use {crate::structs::AxBlockDevice, ax_driver_block::BlockDriverOps};
#[cfg(feature = "display")]
pub use {
    crate::structs::AxDisplayDevice,
    ax_driver_display::{DisplayDriverOps, DisplayInfo},
};
#[cfg(feature = "input")]
pub use {
    crate::structs::AxInputDevice,
    ax_driver_input::{Event, EventType, InputDeviceId, InputDriverOps},
};
#[cfg(feature = "net")]
pub use {
    crate::structs::AxNetDevice,
    ax_driver_net::{NetBufPtr, NetDriverOps},
};
#[cfg(feature = "vsock")]
pub use {
    crate::structs::AxVsockDevice,
    axdriver_vsock::{VsockAddr, VsockConnId, VsockDriverEvent, VsockDriverOps},
};
