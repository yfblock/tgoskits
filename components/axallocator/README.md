# ax-allocator

[![Crates.io](https://img.shields.io/crates/v/ax-allocator.svg?style=flat-square)](https://crates.io/crates/ax-allocator)
[![Documentation](https://docs.rs/ax-allocator/badge.svg?style=flat-square)](https://docs.rs/ax-allocator)
[![License](https://img.shields.io/crates/l/ax-allocator.svg?style=flat-square)](https://crates.io/crates/ax-allocator)

Various allocator algorithms behind a unified interface for `no_std` environments.

## Allocator types

- **Byte-granularity**: [`BuddyByteAllocator`], [`SlabByteAllocator`], [`TlsfByteAllocator`]
- **Page-granularity**: [`BitmapPageAllocator`]
- **ID allocator**: [`IdAllocator`]

[`BuddyByteAllocator`]: https://docs.rs/ax-allocator/latest/ax_allocator/struct.BuddyByteAllocator.html
[`SlabByteAllocator`]: https://docs.rs/ax-allocator/latest/ax_allocator/struct.SlabByteAllocator.html
[`TlsfByteAllocator`]: https://docs.rs/ax-allocator/latest/ax_allocator/struct.TlsfByteAllocator.html
[`BitmapPageAllocator`]: https://docs.rs/ax-allocator/latest/ax_allocator/struct.BitmapPageAllocator.html
[`IdAllocator`]: https://docs.rs/ax-allocator/latest/ax_allocator/trait.IdAllocator.html

## Features

| Feature         | Description                                    |
| --------------- | ---------------------------------------------- |
| `bitmap`        | Bitmap-based page allocator                    |
| `tlsf`          | TLSF byte allocator                            |
| `slab`          | Slab byte allocator (uses `ax_slab_allocator`) |
| `buddy`         | Buddy byte allocator                           |
| `allocator_api` | Implement `Allocator` (nightly)                |
| `page-alloc-*`  | Page size / range (e.g. `page-alloc-256m`)     |
| `ax-errno`       | `AxError` integration                          |

Default: `page-alloc-256m`. Use `full` for all allocators and `allocator_api`.

## Usage

```toml
[dependencies]
ax-allocator = { version = "0.2", features = ["slab", "buddy"] }
```

## License

GPL-3.0-or-later OR Apache-2.0 OR MulanPSL-2.0. See [LICENSE](LICENSE).
