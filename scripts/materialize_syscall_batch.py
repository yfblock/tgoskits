#!/usr/bin/env python3
"""Materialize one rollout batch: contract/*.c, expected/*.line (qemu oracle), matrix partial, catalog.

Usage:
  python3 scripts/materialize_syscall_batch.py --batch B02-file-ops
  python3 scripts/materialize_syscall_batch.py --batch B03-fd-ops --cc /path/to/riscv64-linux-musl-gcc

Requires: PyYAML, qemu-riscv64, riscv64-linux-musl-gcc (override with --cc).
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

_SCRIPTS_DIR = Path(__file__).resolve().parent
if str(_SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(_SCRIPTS_DIR))

try:
    import yaml  # type: ignore
except ImportError:
    yaml = None  # type: ignore

from _materialize_badfd import BADFD_STMT
from _materialize_batch_c import EXTRA_C


def case_tag(probe: str) -> str:
    i = probe.rfind("_")
    if i <= 0:
        return probe
    return probe[:i] + "." + probe[i + 1 :]


def probe_path(syscall: str) -> str:
    return f"/__starryos_probe_{syscall}__/not_there"


def matrix_header_and_entries(matrix_path: Path) -> tuple[str, list]:
    text = matrix_path.read_text(encoding="utf-8")
    m = re.search(r"(?ms)^entries:\s*$(?:\r?\n)", text)
    header = text[: m.start()] if m else ""
    data = yaml.safe_load(text)
    return header, list(data.get("entries") or [])


def write_matrix(matrix_path: Path, header: str, entries: list) -> None:
    body = yaml.dump(
        {"entries": entries},
        allow_unicode=True,
        default_flow_style=False,
        sort_keys=False,
        width=120,
    )
    matrix_path.write_text(header + body, encoding="utf-8")


def catalog_header_and_data(catalog_path: Path) -> tuple[str, dict]:
    text = catalog_path.read_text(encoding="utf-8")
    lines = text.splitlines(keepends=True)
    cut = 0
    for i, ln in enumerate(lines):
        if ln.strip() == "syscalls:":
            cut = i + 1
            break
    header = "".join(lines[:cut])
    data = yaml.safe_load(text)
    return header, data


def write_catalog(catalog_path: Path, header: str, data: dict) -> None:
    rest = yaml.dump(
        data,
        allow_unicode=True,
        default_flow_style=False,
        sort_keys=False,
        width=100,
    )
    # yaml.dump includes top-level keys; strip duplicate dispatch_path if we only want syscalls
    catalog_path.write_text(header + rest.split("syscalls:", 1)[-1], encoding="utf-8")


def write_catalog_full(catalog_path: Path, header_lines: str, data: dict) -> None:
    """Write header (through `syscalls:\\n`) + yaml dump of full document."""
    dumped = yaml.dump(
        data,
        allow_unicode=True,
        default_flow_style=False,
        sort_keys=False,
        width=100,
    )
    catalog_path.write_text(header_lines + dumped.split("syscalls:", 1)[-1], encoding="utf-8")


def fix_catalog_write(catalog_path: Path, data: dict) -> None:
    """Preserve comment header + dispatch_path; rewrite whole file."""
    text = catalog_path.read_text(encoding="utf-8")
    m = re.search(r"(?ms)^(.*?)^syscalls:\s*\n", text)
    if not m:
        raise SystemExit("catalog: could not find syscalls: block")
    header = m.group(1) + "syscalls:\n"
    body = yaml.dump(
        data,
        allow_unicode=True,
        default_flow_style=False,
        sort_keys=False,
        width=100,
    )
    # body is full document starting with dispatch_path - we only want the list under syscalls
    sub = yaml.safe_load(body)
    dumped_syscalls = yaml.dump({"syscalls": sub.get("syscalls", [])}, allow_unicode=True, default_flow_style=False, sort_keys=False, width=100)
    # dumped_syscalls is "syscalls:\n  - ..."
    catalog_path.write_text(header + dumped_syscalls.split("syscalls:\n", 1)[1], encoding="utf-8")


def extract_batch(notes: list) -> str | None:
    for n in notes or []:
        s = str(n)
        if s.startswith("rollout_batch="):
            return s.split("=", 1)[1]
    return None


def extract_pattern(notes: list) -> str:
    for n in notes or []:
        s = str(n)
        if s.startswith("probe_pattern="):
            return s.split("=", 1)[1]
    return ""


def extract_section(notes: list) -> str:
    for n in notes or []:
        s = str(n)
        if s.startswith("section="):
            return s.split("=", 1)[1]
    return "fs"


def impl_path_for_section(section: str) -> str:
    m = {
        "file ops": "os/StarryOS/kernel/src/syscall/fs/",
        "fd ops": "os/StarryOS/kernel/src/syscall/fs/",
        "io": "os/StarryOS/kernel/src/syscall/fs/",
        "fs stat": "os/StarryOS/kernel/src/syscall/fs/",
        "fs mount": "os/StarryOS/kernel/src/syscall/fs/",
        "pipe": "os/StarryOS/kernel/src/syscall/fs/",
        "event": "os/StarryOS/kernel/src/syscall/fs/",
        "pidfd": "os/StarryOS/kernel/src/syscall/fs/",
        "memfd": "os/StarryOS/kernel/src/syscall/fs/",
        "io mpx": "os/StarryOS/kernel/src/syscall/io_mpx/",
        "mm": "os/StarryOS/kernel/src/syscall/mm/",
        "task ops": "os/StarryOS/kernel/src/syscall/task/",
        "task management": "os/StarryOS/kernel/src/syscall/task/",
        "task sched": "os/StarryOS/kernel/src/syscall/task/",
        "task info": "os/StarryOS/kernel/src/syscall/task/",
        "signal": "os/StarryOS/kernel/src/syscall/",
        "signal file descriptors": "os/StarryOS/kernel/src/syscall/fs/",
        "sys": "os/StarryOS/kernel/src/syscall/",
        "time": "os/StarryOS/kernel/src/syscall/",
        "sync": "os/StarryOS/kernel/src/syscall/sync/",
        "msg": "os/StarryOS/kernel/src/syscall/ipc/",
        "shm": "os/StarryOS/kernel/src/syscall/ipc/",
        "net": "os/StarryOS/kernel/src/syscall/net/",
        "dummy fds": "os/StarryOS/kernel/src/syscall/",
    }
    return m.get(section, "os/StarryOS/kernel/src/syscall/")


def domain_for_section(section: str) -> str:
    if "net" in section:
        return "net"
    if section in ("msg", "shm"):
        return "ipc"
    if section == "time":
        return "time"
    if section == "sync":
        return "sync"
    if "task" in section:
        return "task"
    if section in ("signal", "signal file descriptors"):
        return "signal"
    if section == "mm":
        return "mm"
    if section == "io mpx":
        return "io_mpx"
    if section == "sys":
        return "sys"
    return "fs"


# ---------- C emitters: pattern-based ----------

AT_FDCWD_BLOCK = """
#ifndef AT_FDCWD
#define AT_FDCWD (-100)
#endif
"""


def wrap_main(includes: list[str], body: str) -> str:
    inc = "\n".join(f"#include <{h}>" for h in includes)
    return f"/* Generated by materialize_syscall_batch.py */\n{inc}\n{body}\n"


def emit_ret_errno(case: str, ret_fmt: str, ret_expr: str) -> str:
    return f"""
int main(void)
{{
\terrno = 0;
\t{ret_expr};
\tint e = errno;
\tdprintf(1, "CASE {case} {ret_fmt} errno=%d note=handwritten\\n", {ret_fmt.split("=")[0].split()[-1] if "%" in ret_fmt else "((int)(long)r)"}, e);
\treturn 0;
}}
"""


# Simplified: body lines inside main
def c_main(case: str, lines: list[str], ret_name: str = "r", ret_fmt: str = "ret=%d") -> str:
    inner = "\n\t".join(lines)
    return f"""
int main(void)
{{
\terrno = 0;
\t{inner}
\tint e = errno;
\tdprintf(1, "CASE {case} {ret_fmt} errno=%d note=handwritten\\n", (int){ret_name}, e);
\treturn 0;
}}
"""


def c_main_long(case: str, lines: list[str], ret_fmt: str = "ret=%ld") -> str:
    inner = "\n\t".join(lines)
    return f"""
int main(void)
{{
\terrno = 0;
\t{inner}
\tint e = errno;
\tdprintf(1, "CASE {case} {ret_fmt} errno=%d note=handwritten\\n", r, e);
\treturn 0;
}}
"""


def build_source_for_entry(syscall: str, probe: str, pattern: str) -> str | None:
    """Return full C source or None if batch not implemented in this script version."""
    ct = case_tag(probe)
    p = probe_path(syscall)

    if probe in EXTRA_C:
        return EXTRA_C[probe].strip() + "\n"

    # ---- explicit probes (oracle differs from suffix heuristic) ----
    if probe == "close_range_badfd":
        body = """
#include <errno.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>
int main(void)
{
	errno = 0;
	long r = syscall(SYS_close_range, 0xFFFFFFFFu, 10u, 0);
	int e = errno;
	dprintf(1, "CASE close_range.einval ret=%ld errno=%d note=handwritten\\n", r, e);
	return 0;
}
"""
        return body.strip() + "\n"

    if probe == "open_enoent" and pattern == "special":
        body = f"""
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <unistd.h>
static const char p[] = "{p}";
int main(void)
{{
\terrno = 0;
\tint r = open(p, O_RDONLY | O_NOCTTY);
\tint e = errno;
\tdprintf(1, "CASE {ct} ret=%d errno=%d note=handwritten\\n", r, e);
\treturn 0;
}}
"""
        return body.strip() + "\n"

    # ---- errno EFAULT / NULL pointer result ----
    if pattern == "errno_efault_null":
        null_probes: dict[str, tuple[list[str], str, str]] = {
            "clock_getres": (
                ["errno.h", "stdio.h", "time.h"],
                "int r = clock_getres(CLOCK_REALTIME, NULL);",
                "clock_getres.null_ptr",
            ),
            "gettimeofday": (
                ["errno.h", "stdio.h", "sys/time.h"],
                "int r = gettimeofday(NULL, NULL);",
                "gettimeofday.null_tv",
            ),
            "getitimer": (
                ["errno.h", "stdio.h", "sys/time.h"],
                "int r = getitimer(ITIMER_REAL, NULL);",
                "getitimer.null_val",
            ),
            "setitimer": (
                ["errno.h", "stdio.h", "sys/time.h"],
                "int r = setitimer(ITIMER_REAL, NULL, NULL);",
                "setitimer.null_new",
            ),
            "sched_getaffinity": (
                ["errno.h", "stdio.h", "sched.h", "unistd.h"],
                "unsigned long m; int r = sched_getaffinity(0, sizeof(m), NULL);",
                "sched_getaffinity.null_mask",
            ),
            "sched_setaffinity": (
                ["errno.h", "stdio.h", "sched.h", "unistd.h"],
                "unsigned long m = 1; int r = sched_setaffinity(0, sizeof(m), NULL);",
                "sched_setaffinity.null_mask",
            ),
        }
        if syscall not in null_probes:
            return None
        incs, stmt, case_dot = null_probes[syscall]
        body = f"""
int main(void)
{{
\terrno = 0;
\t{stmt}
\tint e = errno;
\tdprintf(1, "CASE {case_dot} ret=%d errno=%d note=handwritten\\n", r, e);
\treturn 0;
}}
"""
        return wrap_main(incs, body)

    # ---- B02-style path ENOENT (also used for B07 path syscalls) ----
    path_calls: dict[str, tuple[list[str], str]] = {
        "chmod": (["errno.h", "stdio.h", "sys/stat.h", "sys/types.h"], "int r = chmod(p, 0777);"),
        "chown": (["errno.h", "stdio.h", "unistd.h"], "int r = chown(p, (uid_t)-1, (gid_t)-1);"),
        "lchown": (["errno.h", "stdio.h", "unistd.h"], "int r = lchown(p, (uid_t)-1, (gid_t)-1);"),
        "readlink": (["errno.h", "stdio.h", "unistd.h"], ""),
        "utime": (["errno.h", "stdio.h", "utime.h"], "int r = utime(p, NULL);"),
        "utimes": (["errno.h", "stdio.h", "sys/time.h"], "int r = utimes(p, NULL);"),
        "access": (["errno.h", "stdio.h", "unistd.h"], "int r = access(p, F_OK);"),
        "stat": (["errno.h", "stdio.h", "sys/stat.h"], "struct stat st; int r = stat(p, &st);"),
        "lstat": (["errno.h", "stdio.h", "sys/stat.h"], "struct stat st; int r = lstat(p, &st);"),
        "truncate": (["errno.h", "stdio.h", "unistd.h"], "int r = truncate(p, 0);"),
        "statx": (
            ["errno.h", "stdio.h", "linux/stat.h", "sys/stat.h", "fcntl.h", "unistd.h"],
            "struct statx stx; int r = statx(AT_FDCWD, p, 0, STATX_BASIC_STATS, &stx);",
        ),
        "statfs": (["errno.h", "stdio.h", "sys/statfs.h"], "struct statfs sf; int r = statfs(p, &sf);"),
        "mount": (
            ["errno.h", "stdio.h", "sys/mount.h"],
            "int r = mount(p, \"/__starryos_probe_mount__/tgt\", \"ext4\", 0, NULL);",
        ),
        "umount2": (["errno.h", "stdio.h", "sys/mount.h"], "int r = umount2(p, 0);"),
    }

    at_calls: dict[str, tuple[list[str], str]] = {
        "fchmodat": (
            ["errno.h", "stdio.h", "fcntl.h", "sys/stat.h"],
            "int r = fchmodat(AT_FDCWD, p, 0777, 0);",
        ),
        "fchmodat2": (
            ["errno.h", "stdio.h", "fcntl.h", "sys/stat.h", "sys/syscall.h", "unistd.h"],
            "int r = (int)syscall(SYS_fchmodat, AT_FDCWD, p, 0777, 0);",
        ),
        "fchownat": (
            ["errno.h", "stdio.h", "fcntl.h", "unistd.h"],
            "int r = fchownat(AT_FDCWD, p, (uid_t)-1, (gid_t)-1, 0);",
        ),
        "readlinkat": (
            ["errno.h", "stdio.h", "fcntl.h", "unistd.h"],
            "char b[256]; ssize_t r = readlinkat(AT_FDCWD, p, b, sizeof(b)); int r2 = (int)r;",
        ),
        "utimensat": (
            ["errno.h", "stdio.h", "fcntl.h", "sys/stat.h"],
            "int r = utimensat(AT_FDCWD, p, NULL, 0);",
        ),
        "faccessat": (["errno.h", "stdio.h", "fcntl.h", "unistd.h"], "int r = faccessat(AT_FDCWD, p, F_OK, 0);"),
        "faccessat2": (
            ["errno.h", "stdio.h", "fcntl.h", "unistd.h", "sys/syscall.h"],
            "int r = (int)syscall(SYS_faccessat, AT_FDCWD, p, F_OK, 0);",
        ),
        "newfstatat": (
            ["errno.h", "stdio.h", "fcntl.h", "sys/stat.h"],
            "struct stat st; int r = fstatat(AT_FDCWD, p, &st, 0);",
        ),
        "fstatat": (
            ["errno.h", "stdio.h", "fcntl.h", "sys/stat.h"],
            "struct stat st; int r = fstatat(AT_FDCWD, p, &st, 0);",
        ),
    }

    if pattern == "errno_enoent_path":
        if syscall not in path_calls:
            return None
        incs, stmt = path_calls[syscall]
        incs = list(incs)
        if syscall == "readlink":
            body = f"""
static const char p[] = "{p}";
int main(void)
{{
\terrno = 0;
\tchar b[256];
\tssize_t r = readlink(p, b, sizeof(b));
\tint e = errno;
\tdprintf(1, "CASE {ct} ret=%d errno=%d note=handwritten\\n", (int)r, e);
\treturn 0;
}}
"""
            return wrap_main(incs, body)
        extra = AT_FDCWD_BLOCK if syscall == "statx" else ""
        body = f"""
{extra}
static const char p[] = "{p}";
int main(void)
{{
\terrno = 0;
\t{stmt}
\tint e = errno;
\tdprintf(1, "CASE {ct} ret=%d errno=%d note=handwritten\\n", (int)r, e);
\treturn 0;
}}
"""
        return wrap_main(incs, body)

    if pattern == "errno_enoent_at":
        if syscall not in at_calls:
            return None
        incs, stmt = at_calls[syscall]
        body = (
            AT_FDCWD_BLOCK
            + f"""
static const char p[] = "{p}";
int main(void)
{{
\terrno = 0;
\t{stmt}
\tint e = errno;
\tdprintf(1, "CASE {ct} ret=%d errno=%d note=handwritten\\n", (int)r, e);
\treturn 0;
}}
"""
        )
        if "readlinkat" in syscall:
            body = (
                AT_FDCWD_BLOCK
                + f"""
static const char p[] = "{p}";
int main(void)
{{
\terrno = 0;
\tchar b[256];
\tssize_t r = readlinkat(AT_FDCWD, p, b, sizeof(b));
\tint e = errno;
\tdprintf(1, "CASE {ct} ret=%d errno=%d note=handwritten\\n", (int)r, e);
\treturn 0;
}}
"""
            )
        return wrap_main(incs, body)

    if pattern == "errno_badfd":
        if syscall not in BADFD_STMT:
            return None
        incs, stmt, _use_long = BADFD_STMT[syscall]
        body = f"""
int main(void)
{{
\terrno = 0;
\t{stmt}
\tint e = errno;
\tdprintf(1, "CASE {ct} ret=%ld errno=%d note=handwritten\\n", (long)r, e);
\treturn 0;
}}
"""
        return wrap_main(incs, body)

    return None


def run_oracle(cc: str, qemu: str, c_src: str) -> str:
    with tempfile.TemporaryDirectory() as td:
        tdp = Path(td)
        src = tdp / "p.c"
        elf = tdp / "p"
        src.write_text(c_src, encoding="utf-8")
        subprocess.run([cc, "-static", "-O2", str(src), "-o", str(elf)], check=True, capture_output=True)
        out = subprocess.run([qemu, str(elf)], capture_output=True, text=True)
        if out.returncode != 0 and out.returncode != 1:
            raise RuntimeError(f"qemu exit {out.returncode}: {out.stderr}")
        for line in (out.stdout + out.stderr).splitlines():
            line = line.strip().replace("\r", "")
            if line.startswith("CASE "):
                return line
    raise RuntimeError("no CASE line in qemu output")


def make_catalog_entry(syscall: str, probe: str, section: str) -> dict:
    rel = f"test-suit/starryos/probes/contract/{probe}.c"
    return {
        "syscall": syscall,
        "domain": domain_for_section(section),
        "dispatch_path": "os/StarryOS/kernel/src/syscall/mod.rs",
        "impl_path": impl_path_for_section(section),
        "status": "partial",
        "semantic_class": "contract_batch",
        "boundary_profiles": ["contract_probe"],
        "core_modes": ["smp1", "smp2"],
        "linux_refs": [f"man 2 {syscall}"],
        "tests": [rel],
        "risk_tags": ["errno_mismatch"],
        "generator_hints": {"template": "contract_errno"},
    }


def main() -> int:
    if yaml is None:
        print("PyYAML required", file=sys.stderr)
        return 2

    ap = argparse.ArgumentParser()
    ap.add_argument("--batch", required=True, help="e.g. B02-file-ops")
    ap.add_argument("--root", type=Path, default=Path("."))
    ap.add_argument("--cc", default=None)
    ap.add_argument("--qemu", default="qemu-riscv64")
    args = ap.parse_args()
    root: Path = args.root.resolve()
    cc = args.cc or shutil.which("riscv64-linux-musl-gcc") or "riscv64-linux-musl-gcc"
    if not shutil.which(cc.split("/")[-1]) and not Path(cc).is_file():
        print(f"Missing cross compiler: {cc}", file=sys.stderr)
        return 1
    if not shutil.which(args.qemu):
        print(f"Missing {args.qemu}", file=sys.stderr)
        return 1

    matrix_path = root / "docs" / "starryos-syscall-compat-matrix.yaml"
    catalog_path = root / "docs" / "starryos-syscall-catalog.yaml"
    contract_dir = root / "test-suit" / "starryos" / "probes" / "contract"
    expected_dir = root / "test-suit" / "starryos" / "probes" / "expected"

    header, entries = matrix_header_and_entries(matrix_path)
    cat_text = catalog_path.read_text(encoding="utf-8")
    cat_data = yaml.safe_load(cat_text)
    existing_syscalls = {str(x.get("syscall")) for x in (cat_data.get("syscalls") or []) if isinstance(x, dict)}

    batch_id = args.batch
    touched: list[str] = []

    for e in entries:
        if not isinstance(e, dict):
            continue
        if str(e.get("parity") or "") != "not_applicable":
            continue
        notes = e.get("notes") or []
        if extract_batch(notes) != batch_id:
            continue
        syscall = str(e["syscall"])
        probe = str(e.get("planned_contract_probe") or "").strip()
        pattern = extract_pattern(notes)
        if not probe:
            print(f"skip {syscall}: no planned_contract_probe", file=sys.stderr)
            continue
        src = build_source_for_entry(syscall, probe, pattern)
        if src is None:
            print(f"skip {syscall}: no emitter for pattern={pattern!r}", file=sys.stderr)
            continue

        line = run_oracle(cc, args.qemu, src)
        (contract_dir / f"{probe}.c").write_text(src, encoding="utf-8")
        (expected_dir / f"{probe}.line").write_text(line + "\n", encoding="utf-8")

        section = extract_section(notes)
        e.clear()
        e["syscall"] = syscall
        e["contract_probe"] = probe
        e["parity"] = "partial"
        e["notes"] = [f"{batch_id} contract probe"]

        if syscall not in existing_syscalls:
            cat_data.setdefault("syscalls", []).append(make_catalog_entry(syscall, probe, section))
            existing_syscalls.add(syscall)

        touched.append(probe)
        print(f"OK {probe} -> {line}")

    if not touched:
        print(f"No entries for batch {batch_id!r}", file=sys.stderr)
        return 4

    write_matrix(matrix_path, header, entries)
    fix_catalog_write(catalog_path, cat_data)
    print(f"Updated matrix + catalog; probes: {len(touched)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
