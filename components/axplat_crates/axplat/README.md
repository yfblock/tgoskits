# ax-plat

[![Crates.io](https://img.shields.io/crates/v/ax-plat)](https://crates.io/crates/ax-plat)
[![Docs.rs](https://docs.rs/ax-plat/badge.svg)](https://docs.rs/ax-plat)
[![CI](https://github.com/arceos-org/axplat_crates/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/arceos-org/axplat_crates/actions/workflows/ci.yml)

This crate provides a unified abstraction layer for diverse hardware platforms. It allows kernel developers to bootstrap custom kernels across various platforms and interact with essential peripherals using hardware-agnostic APIs.

Interfaces can be divided into the following categories:

| Category | Trait       | Description                 |
| -------- | ----------- | --------------------------- |
| init     | `InitIf`    | Platform initialization     |
| console  | `ConsoleIf` | Console input and output    |
| power    | `PowerIf`   | Power management            |
| mem      | `MemIf`     | Physical memory information |
| time     | `TimeIf`    | Time-related operations     |
| irq      | `IrqIf`     | Interrupt request handling  |

Each category of interfaces provides a trait (e.g., `ConsoleIf`) for a platform package to implement. You can use the corresponding platform-related functions in your project directly from the [ax-plat](https://crates.io/crates/ax-plat) crate without importing the specific platform package.

## How to use in your kernel project

```rust
// Link you kernel with the specific platform package in some crate.
// extern crate your_platform_crate;

// Write your kernel code (can be in another crate).
#[ax_plat::main]
fn kernel_main(cpu_id: usize, arg: usize) -> ! {
    // Initialize trap, console, time.
    ax_plat::init::init_early(cpu_id, arg);
    // Initialize platform peripherals (not used in this example).
    ax_plat::init::init_later(cpu_id, arg);

    // Write your kernel code here.
    ax_plat::console_println!("Hello, ArceOS!");

    // Power off the system.
    ax_plat::power::system_off();
}
```

More APIs can be found in the [documentation](https://docs.rs/ax-plat/latest/ax_plat/). More example kernels can be found in the [examples](https://github.com/arceos-org/axplat_crates/tree/main/examples) directory.

## How to write a platform package

#### 1. Implement each interface trait

```rust
use ax_plat::impl_plat_interface;

/// Implementation of Platform initialization.
struct InitIfImpl;

#[impl_plat_interface]
impl ax_plat::init::InitIf for InitIfImpl {
    fn init_early(cpu_id: usize, arg: usize) { /* ... */ }
    fn init_later(cpu_id: usize, arg: usize) { /* ... */ }
    fn init_early_secondary(cpu_id: usize) { /* ... */ }
    fn init_later_secondary(cpu_id: usize) { /* ... */ }
}

/// Implementation of Console input and output.
struct ConsoleIfImpl;

#[impl_plat_interface]
impl ax_plat::console::ConsoleIf for ConsoleIfImpl {
    fn write_bytes(bytes: &[u8]) { /* ... */ }
    fn read_bytes(bytes: &mut [u8]) -> usize { /* ... */ 0 }
    #[cfg(feature = "irq")]
    fn irq_num() -> Option<usize> { None }
}

// Implementation of other traits...
```

#### 2. Implement platform bootstrapping code and call the entry function of ax-plat

```rust
#[unsafe(no_mangle)]
unsafe extern "C" fn __start() -> ! {
    // platform bootstrapping code here.
    /* ... */

    // Call the entry function of ax-plat.
    ax_plat::call_main(0, 0xdeadbeef); // cpu_id = 0, arg = 0xdeadbeef
}
```

We also provide a cargo plugin called [cargo-axplat](https://github.com/arceos-org/axplat_crates/tree/main/cargo-axplat) for creating a new platform package and adding it into your project.

Some examples of platform packages for various platforms are listed in the [platforms](https://github.com/arceos-org/axplat_crates/tree/main/platforms) directory.
