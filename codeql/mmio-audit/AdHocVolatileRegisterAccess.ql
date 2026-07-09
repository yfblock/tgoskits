/**
 * @name Ad-hoc volatile MMIO register access
 * @description Finds every `read_volatile` / `write_volatile` call outside the
 *   canonical `mmio-api` crate. Each hit is a hand-rolled MMIO register
 *   read/write — i.e. a duplicate of the `mmio_read` / `mmio_write` semantics
 *   that bypasses `mmio_api::MmioRaw::read` / `write`.
 * @kind problem
 * @id rs/ad-hoc-volatile-mmio-access
 * @problem.severity recommendation
 */

import rust
import MmioAccess

from Locatable n, string variant, string path
where
  volatileAccess(n, variant) and
  nodePath(n, path) and
  not path.indexOf("memory/mmio-api/") >= 0
select n,
  path + ":" + n.getLocation().getStartLine().toString() +
    "  ad-hoc MMIO access [" + variant + "] — prefer mmio_api::MmioRaw::read/write"
