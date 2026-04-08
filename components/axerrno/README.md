# ax-errno

[![Crates.io](https://img.shields.io/crates/v/ax-errno)](https://crates.io/crates/ax-errno)
[![Docs.rs](https://docs.rs/ax-errno/badge.svg)](https://docs.rs/ax-errno)
[![CI](https://github.com/arceos-org/axerrno/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/arceos-org/axerrno/actions/workflows/ci.yml)

Generic error code representation.

It provides two error types and the corresponding result types:

- [`AxError`] and [`AxResult`]: A generic error type similar to
  [`std::io::ErrorKind`].
- [`LinuxError`] and [`LinuxResult`]: Linux specific error codes defined in
  `errno.h`. It can be converted from [`AxError`].

[`AxError`]: https://docs.rs/ax-errno/latest/ax-errno/enum.AxError.html
[`AxResult`]: https://docs.rs/ax-errno/latest/ax-errno/type.AxResult.html
[`LinuxError`]: https://docs.rs/ax-errno/latest/ax-errno/enum.LinuxError.html
[`LinuxResult`]: https://docs.rs/ax-errno/latest/ax-errno/type.LinuxResult.html
[`std::io::ErrorKind`]: https://doc.rust-lang.org/std/io/enum.ErrorKind.html
