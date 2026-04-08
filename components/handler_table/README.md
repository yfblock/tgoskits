# ax-handler-table

[![Crates.io](https://img.shields.io/crates/v/ax-handler-table)](https://crates.io/crates/ax-handler-table)
[![Docs.rs](https://docs.rs/ax-handler-table/badge.svg)](https://docs.rs/ax-handler-table)
[![CI](https://github.com/arceos-org/ax-handler-table/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/arceos-org/ax-handler-table/actions/workflows/ci.yml)

A lock-free table of event handlers.

## Examples

```rust
use ax_handler_table::HandlerTable;

static TABLE: HandlerTable<8> = HandlerTable::new();

TABLE.register_handler(0, || {
   println!("Hello, event 0!");
});
TABLE.register_handler(1, || {
   println!("Hello, event 1!");
});

assert!(TABLE.handle(0)); // print "Hello, event 0!"
assert!(!TABLE.handle(2)); // unregistered

assert!(TABLE.unregister_handler(2).is_none());
let func = TABLE.unregister_handler(1).unwrap(); // retrieve the handler
func(); // print "Hello, event 1!"

assert!(!TABLE.handle(1)); // unregistered
```
