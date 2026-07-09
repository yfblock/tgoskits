/**
 * Shared helpers for auditing ad-hoc MMIO register read/write implementations.
 *
 * Uses only the upstream `codeql/rust-all` library (`import rust`) high-level
 * API, so the selected entities (`CallExpr`, `MethodCallExpr`, `LiteralExpr`)
 * carry proper source locations and render with file:line in CSV / SARIF.
 *
 *   - variant A (free function): `core::ptr::read_volatile` / `write_volatile`
 *     detected via `CallExpr.getFunction().(PathExpr).getPath().getText()`
 *   - variant B (method form):   `ptr.read_volatile()` / `ptr.write_volatile()`
 *     detected via `MethodCallExpr.getIdentifier().getText()`
 *   - variant C (inline asm, best-effort): string literals whose text contains
 *     a load/store mnemonic, via `LiteralExpr.getTextValue()`
 */

import rust
import codeql.rust.elements.Locatable
import codeql.rust.elements.CallExpr
import codeql.rust.elements.MethodCallExpr
import codeql.rust.elements.PathExpr
import codeql.rust.elements.LiteralExpr

/** Method calls whose method name equals `name`
 *  (e.g. `ptr.read_volatile()`, `ptr.write_volatile()`). */
predicate methodCallNamed(MethodCallExpr m, string name) {
  name = m.getIdentifier().getText()
}

/** Free-function calls whose called path's last segment equals `name`
 *  (e.g. `core::ptr::read_volatile`, `core::ptr::write_volatile`). */
predicate freeFnCallNamed(CallExpr c, string name) {
  name = c.getFunction().(PathExpr).getPath().getText()
}

/**
 * An AST node that performs a volatile MMIO register read or write, tagged
 * with the syntactic variant:
 *   - variant A: free-function form  `core::ptr::read_volatile` / `write_volatile`
 *   - variant B: method form         `ptr.read_volatile()`      / `ptr.write_volatile()`
 */
predicate volatileAccess(Locatable n, string variant) {
  exists(CallExpr c |
    freeFnCallNamed(c, "read_volatile") and n = c and variant = "A: core::ptr::read_volatile"
  )
  or
  exists(CallExpr c |
    freeFnCallNamed(c, "write_volatile") and n = c and variant = "A: core::ptr::write_volatile"
  )
  or
  exists(MethodCallExpr m |
    methodCallNamed(m, "read_volatile") and n = m and variant = "B: *.read_volatile()"
  )
  or
  exists(MethodCallExpr m |
    methodCallNamed(m, "write_volatile") and n = m and variant = "B: *.write_volatile()"
  )
}

/**
 * A string-literal whose text contains an architecture load/store mnemonic,
 * paired with the matched mnemonic. Best-effort signal for inline-assembly
 * MMIO access (variant C) such as `core::arch::asm!("str ...", ...)`.
 */
predicate literalMnemonic(LiteralExpr lit, string mnemonic) {
  exists(string text |
    text = lit.getTextValue() and
    (
      mnemonic = "str" and text.regexpMatch("(^|[^a-z0-9_])str([^a-z0-9_]|$)")
      or
      mnemonic = "strb" and text.regexpMatch("(^|[^a-z0-9_])strb([^a-z0-9_]|$)")
      or
      mnemonic = "strh" and text.regexpMatch("(^|[^a-z0-9_])strh([^a-z0-9_]|$)")
      or
      mnemonic = "ldr" and text.regexpMatch("(^|[^a-z0-9_])ldr([^a-z0-9_]|$)")
      or
      mnemonic = "ldrb" and text.regexpMatch("(^|[^a-z0-9_])ldrb([^a-z0-9_]|$)")
      or
      mnemonic = "ldrh" and text.regexpMatch("(^|[^a-z0-9_])ldrh([^a-z0-9_]|$)")
      or
      mnemonic = "ldur" and text.regexpMatch("(^|[^a-z0-9_])ldur([^a-z0-9_]|$)")
      or
      mnemonic = "stur" and text.regexpMatch("(^|[^a-z0-9_])stur([^a-z0-9_]|$)")
    )
  )
}

/** Absolute source path of a locatable node. */
predicate nodePath(Locatable n, string path) {
  path = n.getLocation().getFile().getAbsolutePath()
}
