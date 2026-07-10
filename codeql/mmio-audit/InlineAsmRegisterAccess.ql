/**
 * @name Inline-asm MMIO register access (variant C, best-effort)
 * @description Finds string literals containing an architecture load/store
 *   mnemonic (str/strb/strh/ldr/ldrb/ldrh/ldur/stur) — the hallmark of an
 *   inline-assembly MMIO register access such as
 *   `core::arch::asm!("str {value:w}, [{addr}]", ...)` in `arm-gic-driver`.
 *
 *   CAVEAT: This CodeQL install's Rust extractor does not retain token-tree
 *   text; the `asm!` template is only reachable if the extractor also recorded
 *   it as a `literal_expr`. If a given build records no such literal, the hit
 *   is missed — cross-check with the grep snapshot in README.md. The query is
 *   deliberately broad (manual review) rather than precise.
 * @kind problem
 * @id rs/inline-asm-mmio-access
 * @problem.severity recommendation
 */

import rust
import MmioAccess

from LiteralExpr lit, string mnemonic, string path
where
  literalMnemonic(lit, mnemonic) and
  nodePath(lit, path) and
  not path.indexOf("memory/mmio-api/") >= 0
select lit,
  path + ":" + lit.getLocation().getStartLine().toString() +
    "  possible inline-asm MMIO access [mnemonic=" + mnemonic + "] (variant C) — prefer mmio_api::MmioRaw::read/write"
