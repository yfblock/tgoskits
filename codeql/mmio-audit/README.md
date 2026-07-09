# MMIO read/write audit (CodeQL)

This CodeQL query pack records every **ad-hoc memory-mapped I/O (MMIO) register
read/write** implementation in the workspace, so that new duplicates of the
`mmio_read` / `mmio_write` semantics can be detected continuously.

The workspace already ships a single, blessed MMIO abstraction —
[`mmio_api::MmioRaw::read` / `write`](../../memory/mmio-api/src/lib.rs) — but
many driver / component / virtualization crates hand-roll their own volatile
register access instead of going through it. These queries surface all of those
hand-rolled sites.

## Current snapshot

Built and run against `feat/codeql` (2026-07-09) via
`codeql database create --language=rust --command='cargo build -p mmio-api'`
(the Rust extractor scans the whole workspace source regardless of the package
built) and `codeql database analyze ... --search-path=codeql/codeql-lib`.

| Variant | Pattern | Hits |
|---|---|---|
| **A** — free function | `core::ptr::read_volatile` / `write_volatile` | 99 |
| **B** — method form | `ptr.read_volatile()` / `ptr.write_volatile()` | 193 |
| **C** — inline asm | `core::arch::asm!("str …", …)` | 0 (see caveat) |
| **Total ad-hoc** | (outside `memory/mmio-api/`) | **292** across **70 files** |
| Canonical (excluded) | `mmio_api::MmioRaw::read/write` | 2 |

Top files by ad-hoc hit count:

| Hits | File |
|---:|---|
| 27 | `drivers/blk/dwmmc-host/src/lib.rs` |
| 16 | `os/StarryOS/kernel/src/pseudofs/dev/cvi_usb_camera.rs` |
| 15 | `drivers/usb/usb-host/src/backend/kmod/xhci/host.rs` |
| 12 | `virtualization/arm_vgic/src/v3/gits.rs` |
| 11 | `drivers/blk/dwmmc-host/src/dma.rs` |
| 11 | `drivers/soc/rockchip/rockchip-soc/src/variants/rk3588/pinctrl/reg.rs` |
| 10 | `drivers/rdrive/src/probe/acpi.rs` |
|  9 | `drivers/blk/phytium-mci-host/src/dma.rs` |
|  9 | `drivers/npu/rockchip-npu/src/lib.rs` |
|  8 | `drivers/net/fxmac_rs/src/fxmac_dma.rs` |
|  8 | `virtualization/arm_vgic/src/v3/utils.rs` |
|  8 | `virtualization/riscv_vplic/src/utils.rs` |

Named ad-hoc helper wrappers (each is a duplicate of `mmio_read`/`mmio_write`
semantics and a migration target for `mmio-api`):

| Wrapper | Location |
|---|---|
| `mmio_read<T>` / `mmio_write<T>` | `components/sdhci-cv1800/src/lib.rs:36,40` |
| `mmio_write32` / `mmio_write8` (inline asm) | `drivers/intc/arm-gic-driver/src/version/v3/gicd.rs:110,122` |
| `perform_mmio_read` / `perform_mmio_write` | `virtualization/arm_vgic/src/v3/utils.rs:21,32` |
| `perform_mmio_read` / `perform_mmio_write` | `virtualization/riscv_vplic/src/utils.rs:14,26` |
| `read_reg` / `write_reg` | `drivers/net/fxmac_rs/src/fxmac.rs:216,228` |
| `read_reg` / `write_reg` | `drivers/npu/k230-kpu/src/lib.rs:142,146` |
| `read_reg` / `write_reg` | `drivers/pwm/rockchip-pwm/src/lib.rs:67,71` |
| `read_reg` / `write_reg` | `drivers/serial/some-serial/src/ns16550/mmio.rs:16,23` |

### Variant C caveat (inline asm)

`drivers/intc/arm-gic-driver/src/version/v3/gicd.rs` implements `mmio_write32` /
`mmio_write8` with `core::arch::asm!("str …", …)` / `"strb …"`. The CodeQL Rust
extractor in this install does **not** retain inline-asm template text as a
`literal_expr`, so `InlineAsmRegisterAccess.ql` returns 0 on the current
database. That query is kept as a forward-looking check (it will fire if a
future extractor records the template), and the known site is tracked in the
table above and via:

```
grep -rnE '"(str|strb|strh|ldr|ldrb|ldrh|ldur|stur)(\{| |,|\[)' --include='*.rs' .
```

(That grep also matches non-MMIO inline asm such as boot-time zeroing in
`platforms/someboot/.../entry.rs` and context save/restore in
`virtualization/arm_vcpu/...` — review manually.)

## Files

| File | Purpose |
|---|---|
| `qlpack.yml` | Query pack; depends on `codeql/rust-all`. |
| `MmioAccess.qll` | Shared predicates (variant classification, path helpers). |
| `AdHocVolatileRegisterAccess.ql` | Main query — variants A + B, excludes `mmio-api`. |
| `InlineAsmRegisterAccess.ql` | Best-effort query — variant C. |
| `mmio-audit.qls` | Query suite listing both real queries. |
| `../fetch-codeql-lib.sh` | Sparse-clones `github/codeql` so the queries can compile locally. |
| `../run-mmio-audit.sh` | Builds the DB and runs the queries end-to-end (local). |

## CI

`.github/workflows/mmio-audit.yml` runs the audit on push to `main` and on pull
requests whose changed paths include `**/*.rs`, `codeql/**`, or the workflow
itself. It uses `github/codeql-action/init@v3` with `languages: rust`,
`build-mode: none`, and `config.queries: ./codeql/mmio-audit` (so only this pack
runs — default CodeQL queries are disabled), then `cargo build -p mmio-api` for
extraction, then `github/codeql-action/analyze@v3`, uploading SARIF to the
repo's **Security** tab. In CI the `codeql/rust-all` dependency is resolved from
the CodeQL CLI bundle, so no local `codeql-lib` clone is needed.

A build is required (not `build-mode: none` source-only): a no-build database
misses several crates that contain ad-hoc MMIO access
(`sdhci-cv1800`, `arm-gic-driver`, `dwmmc-host`, …), so `cargo build -p mmio-api`
under the extractor is what gives full workspace coverage.

## Requirements (local runs)

The locally installed CodeQL CLI (`/opt/codeql`, v2.24.3) ships only the Rust
**extractor + raw dbscheme** — it has no `codeql/rust-all` query library, and
the library cannot be downloaded from the GitHub Container Registry in this
environment (HTTP 403). The queries therefore depend on `codeql/rust-all`,
which is provided by sparse-cloning `github.com/github/codeql`:

```sh
codeql/fetch-codeql-lib.sh        # one-time: clones into codeql/codeql-lib/
```

(CI does not need this — the CodeQL Action bundle already contains
`codeql/rust-all`.)

## Run (local)

```sh
codeql/run-mmio-audit.sh          # builds DB at /tmp/tgoskits-mmio-db, writes
                                   # codeql/mmio-audit-results.csv
```

Or manually:

```sh
# 1. database (extractor scans the whole workspace source)
codeql database create /tmp/tgoskits-mmio-db \
  --language=rust --source-root=. --command='cargo build -p mmio-api' --overwrite

# 2. queries (both .ql files → one combined CSV)
codeql database analyze /tmp/tgoskits-mmio-db \
  codeql/mmio-audit/AdHocVolatileRegisterAccess.ql \
  codeql/mmio-audit/InlineAsmRegisterAccess.ql \
  --search-path=codeql/codeql-lib --format=csv --output=results.csv
```

## Ongoing detection

Run `codeql/run-mmio-audit.sh` (or step 2 above) on a fresh database after
changes. Any **new** `read_volatile` / `write_volatile` call outside
`memory/mmio-api/` is a new ad-hoc MMIO implementation and shows up as a new
row; the count should trend down as code migrates to `mmio_api::MmioRaw`.
