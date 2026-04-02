#!/usr/bin/env python3
"""Generate Markdown inventory docs from dispatch JSON (+ optional mod.rs / catalog / matrix).

Steps (for separate commits):
  --step 1  syscall, section, cfgs only -> docs/starryos-syscall-dispatch-table.md
  --step 2  + handler (from mod.rs), catalog flag, impl_path -> docs/starryos-syscall-dispatch-handlers.md
  --step 3  + catalog probe basenames, matrix parity -> docs/starryos-syscall-behavior-evidence.md
  --step all  write all three files (full pipeline)
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

try:
    import yaml  # type: ignore
except ImportError:
    yaml = None  # type: ignore


def _split_match_block(text: str) -> str | None:
    needle = "let result = match sysno {"
    start = text.find(needle)
    if start < 0:
        return None
    start += len(needle)
    end = text.find("\n        _ => {", start)
    if end < 0:
        end = text.find("\n        _ =>", start)
    if end < 0:
        return None
    return text[start:end]


def normalize_match_block(block: str) -> str:
    """Join `\\n        | Sysno::` continuation lines into one arm (see timerfd_create arm)."""
    return re.sub(r"\n\s+\|\s+(?=Sysno::)", " | ", block)


def handlers_by_syscall(block: str) -> dict[str, str]:
    """Map syscall name -> primary Rust handler fn or sentinel."""
    block = normalize_match_block(block)
    arm_start = re.compile(
        r"(?m)^\s*(Sysno::\w+(?:\s*\|\s*Sysno::\w+)*)\s*=>\s*",
    )
    end_anchor = "\n        _ => {"
    end_i = block.find(end_anchor)
    if end_i < 0:
        end_i = len(block)
    block = block[:end_i]
    matches = list(arm_start.finditer(block))
    out: dict[str, str] = {}
    for i, m in enumerate(matches):
        chunk_end = matches[i + 1].start() if i + 1 < len(matches) else len(block)
        chunk = block[m.start() : chunk_end]
        names = re.findall(r"Sysno::(\w+)", m.group(1))
        h = re.search(r"\b(sys_\w+)\s*\(", chunk)
        if h:
            handler = h.group(1)
        elif "Ok(0)" in chunk:
            handler = "Ok(0)"
        elif "sys_dummy_fd" in chunk:
            handler = "sys_dummy_fd"
        else:
            handler = ""
        for n in names:
            out[n] = handler
    return out


def md_cell(s: str) -> str:
    return (s or "—").replace("|", "\\|").replace("\n", " ")


def load_catalog(root: Path) -> dict[str, dict]:
    if yaml is None:
        return {}
    p = root / "docs/starryos-syscall-catalog.yaml"
    data = yaml.safe_load(p.read_text(encoding="utf-8"))
    entries = data.get("syscalls") or []
    m: dict[str, dict] = {}
    for e in entries:
        if isinstance(e, dict) and "syscall" in e:
            m[str(e["syscall"])] = e
    return m


def load_matrix(root: Path) -> dict[str, dict]:
    if yaml is None:
        return {}
    p = root / "docs/starryos-syscall-compat-matrix.yaml"
    data = yaml.safe_load(p.read_text(encoding="utf-8"))
    entries = data.get("entries") or []
    m: dict[str, dict] = {}
    for e in entries:
        if isinstance(e, dict) and "syscall" in e:
            m[str(e["syscall"])] = e
    return m


def probe_basenames_from_catalog_tests(tests: list) -> str:
    out = []
    for t in tests or []:
        stem = Path(str(t)).stem
        out.append(stem)
    return ", ".join(out) if out else "—"


def write_dispatch_table(rows: list[dict], path: Path, title: str, intro: str) -> None:
    lines = [
        title,
        "",
        intro,
        "",
        f"**条目数**: {len(rows)}",
        "",
        "| # | syscall | section | cfgs |",
        "|---|---------|---------|------|",
    ]
    for i, r in enumerate(rows, 1):
        cfgs = "; ".join(r.get("cfgs") or []) or "—"
        lines.append(
            f"| {i} | `{md_cell(r['syscall'])}` | {md_cell(r.get('section_comment') or '')} | {md_cell(cfgs)} |"
        )
    lines.append("")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines), encoding="utf-8")


def write_handlers_table(
    rows: list[dict],
    handlers: dict[str, str],
    catalog: dict[str, dict],
    path: Path,
) -> None:
    lines = [
        "# StarryOS 分发表 + mod.rs 入口函数（handler）",
        "",
        "由 `scripts/render_starry_syscall_inventory.py --step 2` 生成。"
        "\n\n**handler** 自 `handle_syscall` 的 `match` 臂解析（块形式 `=> { ... }` 取首个 `sys_*` 调用）。",
        "",
        f"**条目数**: {len(rows)}",
        "",
        "| # | syscall | section | cfgs | handler | in_catalog | impl_path |",
        "|---|---------|---------|------|----------|------------|-----------|",
    ]
    for i, r in enumerate(rows, 1):
        name = r["syscall"]
        cfgs = "; ".join(r.get("cfgs") or []) or "—"
        h = handlers.get(name, "")
        cat = catalog.get(name)
        in_cat = "yes" if cat else "—"
        impl = ""
        if cat:
            impl = str(cat.get("impl_path") or "")
        lines.append(
            f"| {i} | `{md_cell(name)}` | {md_cell(r.get('section_comment') or '')} | {md_cell(cfgs)} "
            f"| `{md_cell(h)}` | {in_cat} | {md_cell(impl)} |"
        )
    lines.append("")
    path.write_text("\n".join(lines), encoding="utf-8")


def write_behavior_table(
    rows: list[dict],
    handlers: dict[str, str],
    catalog: dict[str, dict],
    matrix: dict[str, dict],
    path: Path,
) -> None:
    lines = [
        "# StarryOS syscall 行为证据（Linux oracle / guest 矩阵）",
        "",
        "由 `scripts/render_starry_syscall_inventory.py --step 3` 生成。",
        "",
        "- **matrix_probe**：兼容矩阵 [docs/starryos-syscall-compat-matrix.yaml](docs/starryos-syscall-compat-matrix.yaml) 中的 `contract_probe`（主行）。",
        "- **catalog_probes**：catalog `tests:` 中的 contract 文件名（不含路径）。",
        "- **matrix_parity**：矩阵 `parity`（无行则为 —）。",
        "",
        "全量 **Linux user oracle**：`VERIFY_STRICT=1 test-suit/starryos/scripts/run-diff-probes.sh verify-oracle-all`。",
        "全量 **SMP2 guest vs oracle**：`test-suit/starryos/scripts/run-smp2-guest-matrix.sh`。",
        "",
        f"**分发表条目数**: {len(rows)}",
        "",
        "| syscall | handler | matrix_parity | matrix_probe | catalog_probes |",
        "|---------|---------|---------------|--------------|----------------|",
    ]
    dispatch_names = {r["syscall"] for r in rows}
    for r in rows:
        name = r["syscall"]
        h = handlers.get(name, "")
        mat = matrix.get(name)
        parity = str(mat.get("parity", "")) if mat else "—"
        mprobe = str(mat.get("contract_probe", "") or "").strip() if mat else ""
        mprobe = mprobe or "—"
        cat = catalog.get(name)
        cprobes = probe_basenames_from_catalog_tests(cat.get("tests") if cat else [])
        lines.append(
            f"| `{md_cell(name)}` | `{md_cell(h)}` | {md_cell(parity)} | {md_cell(mprobe)} | {md_cell(cprobes)} |"
        )
    lines.append("")
    extra = sorted(k for k in matrix if k not in dispatch_names)
    if extra:
        lines.extend(
            [
                "## 兼容矩阵中有、但不在分发表 JSON 中的条目",
                "",
                "| syscall | matrix_parity | matrix_probe | notes |",
                "|---------|---------------|--------------|-------|",
            ]
        )
        for k in extra:
            mat = matrix[k]
            parity = str(mat.get("parity", ""))
            mprobe = str(mat.get("contract_probe", "") or "").strip() or "—"
            notes = "; ".join(mat.get("notes") or []) or "—"
            lines.append(f"| `{md_cell(k)}` | {md_cell(parity)} | {md_cell(mprobe)} | {md_cell(notes)} |")
        lines.append("")
    path.write_text("\n".join(lines), encoding="utf-8")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", type=Path, default=Path("."))
    ap.add_argument(
        "--json",
        type=Path,
        default=Path("docs/starryos-syscall-dispatch.json"),
    )
    ap.add_argument("--mod-rs", type=Path, default=Path("os/StarryOS/kernel/src/syscall/mod.rs"))
    ap.add_argument("--step", choices=("1", "2", "3", "all"), default="all")
    args = ap.parse_args()
    root = args.root.resolve()
    js_path = (root / args.json).resolve() if not args.json.is_absolute() else args.json
    if not js_path.is_file():
        print(f"Missing {js_path}; run: python3 scripts/extract_starry_syscalls.py --out-json {args.json}", file=sys.stderr)
        return 1
    payload = json.loads(js_path.read_text(encoding="utf-8"))
    rows: list[dict] = payload.get("syscalls") or []

    mod_path = (root / args.mod_rs).resolve() if not args.mod_rs.is_absolute() else args.mod_rs
    mod_text = mod_path.read_text(encoding="utf-8")
    block = _split_match_block(mod_text)
    handlers = handlers_by_syscall(block) if block else {}

    catalog = load_catalog(root) if yaml is not None else {}
    matrix = load_matrix(root) if yaml is not None else {}

    out1 = root / "docs/starryos-syscall-dispatch-table.md"
    out2 = root / "docs/starryos-syscall-dispatch-handlers.md"
    out3 = root / "docs/starryos-syscall-behavior-evidence.md"

    if args.step in ("1", "all"):
        write_dispatch_table(
            rows,
            out1,
            "# StarryOS 系统调用分发表（机器生成）",
            "数据源：[docs/starryos-syscall-dispatch.json](docs/starryos-syscall-dispatch.json)（"
            "`python3 scripts/extract_starry_syscalls.py --out-json ...`）。"
            " 表示 `handle_syscall` 中**已挂接**的 `Sysno`；`cfgs` 非空时仅在对应 **target/feature** 下参与编译。",
        )
        print(f"Wrote {out1}")

    if args.step in ("2", "all"):
        if yaml is None:
            print("PyYAML missing; step 2 needs catalog YAML", file=sys.stderr)
            return 1
        catalog = load_catalog(root)
        write_handlers_table(rows, handlers, catalog, out2)
        print(f"Wrote {out2}")

    if args.step in ("3", "all"):
        if yaml is None:
            print("PyYAML missing; step 3 needs YAML", file=sys.stderr)
            return 1
        catalog = load_catalog(root)
        matrix = load_matrix(root)
        write_behavior_table(rows, handlers, catalog, matrix, out3)
        print(f"Wrote {out3}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
