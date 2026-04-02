#!/usr/bin/env python3
"""Generate syscall probe rollout batches + merge compat matrix scaffold rows.

Reads:
  docs/starryos-syscall-dispatch.json
  os/StarryOS/kernel/src/syscall/mod.rs (handlers)

Writes:
  docs/starryos-syscall-probe-rollout.yaml — batches, planned probes, suggested case
  docs/starryos-syscall-compat-matrix.yaml — merges new rows (preserves partial/aligned/divergent)

Rules:
  - Syscalls already in matrix with parity partial|aligned|divergent are left unchanged.
  - Others get parity not_applicable + planned_contract_probe (+ rollout_batch in notes).
  - check_compat_matrix.py ignores not_applicable rows (no contract files required until promoted).

After running:
  python3 scripts/render_starry_syscall_inventory.py --step 3
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

try:
    import yaml  # type: ignore
except ImportError:
    yaml = None  # type: ignore

# Already have contract + expected + matrix parity partial (do not regenerate).
DONE_PARTIAL = frozenset(
    {
        "ioctl",
        "unlink",
        "getcwd",
        "openat",
        "close",
        "dup",
        "fcntl",
        "read",
        "write",
        "lseek",
        "ppoll",
        "pipe2",
        "clock_gettime",
        "execve",
        "wait4",
        "futex",
    }
)


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


def handlers_by_syscall(mod_text: str) -> dict[str, str]:
    block = _split_match_block(mod_text)
    if not block:
        return {}
    block = re.sub(r"\n\s+\|\s+(?=Sysno::)", " | ", block)
    arm_start = re.compile(r"(?m)^\s*(Sysno::\w+(?:\s*\|\s*Sysno::\w+)*)\s*=>\s*")
    matches = list(arm_start.finditer(block))
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
        handler = h.group(1) if h else ""
        if not handler and "Ok(0)" in chunk:
            handler = "Ok(0)"
        elif not handler and "sys_dummy_fd" in chunk:
            handler = "sys_dummy_fd"
        for n in names:
            out[n] = handler
    return out


# First argument is fd; Linux returns EBADF for -1 on riscv64 glibc/musl for these (spot-checked / man 2).
FD_BADFD_FIRST = frozenset(
    {
        "fchdir",
        "fsync",
        "fdatasync",
        "ftruncate",
        "fchmod",
        "fchown",
        "getdents64",
        "flock",
        "fstat",
        "fstatfs",
        "syncfs",
        "fadvise64",
        "pread64",
        "pwrite64",
        "preadv",
        "pwritev",
        "preadv2",
        "pwritev2",
        "sendfile",
        "copy_file_range",
        "splice",
        "readv",
        "writev",
        "fallocate",
        "close_range",
        "dup2",
        "dup3",
        "epoll_ctl",
        "epoll_pwait",
        "epoll_pwait2",
        "accept",
        "accept4",
        "shutdown",
        "getsockname",
        "getpeername",
        "listen",
        "recvfrom",
        "sendto",
        "recvmsg",
        "sendmsg",
        "getsockopt",
        "setsockopt",
        "pidfd_getfd",
        "pidfd_send_signal",
    }
)

# Pathname string: non-existent absolute path -> ENOENT (pattern like unlink_enoent).
PATH_ENOENT = frozenset(
    {
        "chdir",
        "chroot",
        "mkdir",
        "link",
        "rmdir",
        "symlink",
        "rename",
        "truncate",
        "access",
        "stat",
        "lstat",
        "statx",
        "readlink",
        "chmod",
        "chown",
        "lchown",
        "utime",
        "utimes",
        "mount",
        "umount2",
    }
)

# dirfd + path: use AT_FDCWD + missing path -> ENOENT.
AT_PATH_ENOENT = frozenset(
    {
        "mkdirat",
        "linkat",
        "unlinkat",
        "symlinkat",
        "renameat",
        "renameat2",
        "readlinkat",
        "fchmodat",
        "fchmodat2",
        "fchownat",
        "utimensat",
        "faccessat",
        "faccessat2",
        "newfstatat",
        "fstatat",
    }
)

# timespec / struct pointer NULL -> EFAULT (like clock_gettime_null_ts).
NULL_STRUCT_EFAULT = frozenset(
    {
        "clock_getres",
        "gettimeofday",
        "sched_getaffinity",
        "sched_setaffinity",
        "getitimer",
        "setitimer",
    }
)

SPECIAL_PROBE: dict[str, tuple[str, str, str]] = {
    # (batch_id, planned_contract_probe, suggested_case one-liner)
    "socket": ("B18-net", "socket_invalid_domain", "invalid socket domain -> EINVAL/EAFNOSUPPORT"),
    "socketpair": ("B18-net", "socketpair_einval", "invalid family -> EINVAL"),
    "bind": ("B18-net", "bind_badfd", "sockfd=-1 -> EBADF"),
    "connect": ("B18-net", "connect_badfd", "sockfd=-1 -> EBADF"),
    "mmap": (
        "B06-mm",
        "mmap_nonanon_badfd",
        "MAP_ANONYMOUS cleared + fd=-1 -> EBADF on Linux; flags TBD in contract",
    ),
    "brk": ("B06-mm", "brk_increment_smoke", "increment 0 returns current brk; compare shape"),
    "munmap": ("B06-mm", "munmap_einval", "NULL len mismatch -> EINVAL"),
    "mprotect": ("B06-mm", "mprotect_einval", "NULL len 0 -> EINVAL"),
    "mincore": ("B06-mm", "mincore_efault", "invalid vec -> EFAULT"),
    "mremap": ("B06-mm", "mremap_einval", "invalid old_address -> EINVAL"),
    "madvise": ("B06-mm", "madvise_einval", "invalid advice -> EINVAL"),
    "msync": ("B06-mm", "msync_einval", "invalid flags -> EINVAL"),
    "mlock": ("B06-mm", "mlock_enomem", "unmapped range -> ENOMEM"),
    "mlock2": ("B06-mm", "mlock2_einval", "bad flags -> EINVAL"),
    "sync": ("B01-fs-ctl", "sync_void_smoke", "void syscall; serializable no-op line"),
    "clone": ("B10-task-mgmt", "clone_errno_probe", "invalid flags -> EINVAL (minimal)"),
    "clone3": ("B10-task-mgmt", "clone3_errno_probe", "NULL attr -> EFAULT/EINVAL"),
    "fork": ("B10-task-mgmt", "fork_smoke_v1", "child exits 0; host/guest isolation heavy"),
    "exit": ("B10-task-mgmt", "exit_smoke_v1", "exit(0); last thread"),
    "exit_group": ("B10-task-mgmt", "exit_group_smoke_v1", "exit_group(0)"),
    "rt_sigreturn": ("B13-signal", "rt_sigreturn_probe_tbd", "return path; not a simple errno probe"),
    "rt_sigsuspend": ("B13-signal", "rt_sigsuspend_probe_tbd", "blocks; needs signal delivery"),
    "rt_sigtimedwait": ("B13-signal", "rt_sigtimedwait_probe_tbd", "timeout 0 no matching sig"),
    "riscv_flush_icache": ("B14-sys", "riscv_flush_icache_einval", "NULL range -> EINVAL/EFAULT"),
    "syslog": ("B14-sys", "syslog_bad_type", "invalid type -> EINVAL"),
    "seccomp": ("B14-sys", "seccomp_einval", "invalid op -> EINVAL"),
    "membarrier": ("B16-sync", "membarrier_einval", "invalid cmd -> EINVAL"),
    "msgget": ("B17-ipc", "msgget_einval", "invalid key/flags"),
    "msgsnd": ("B17-ipc", "msgsnd_badid", "invalid msqid -> EINVAL/EIDRM"),
    "msgrcv": ("B17-ipc", "msgrcv_badid", "invalid msqid"),
    "msgctl": ("B17-ipc", "msgctl_badid", "invalid msqid"),
    "shmget": ("B17-ipc", "shmget_einval", "invalid size/key"),
    "shmat": ("B17-ipc", "shmat_badid", "invalid shmid"),
    "shmctl": ("B17-ipc", "shmctl_badid", "invalid shmid"),
    "shmdt": ("B17-ipc", "shmdt_einval", "invalid addr -> EINVAL"),
    "open": ("B03-fd-ops", "open_enoent", "O_RDONLY on missing abs path -> ENOENT"),
    "eventfd2": ("B08-vfs-special", "eventfd2_einval", "invalid flags -> EINVAL"),
    "memfd_create": ("B08-vfs-special", "memfd_create_einval", "invalid flags -> EINVAL"),
    "pidfd_open": ("B08-vfs-special", "pidfd_open_esrch", "nonexistent pid -> ESRCH"),
    "epoll_create1": ("B05-io-mpx", "epoll_create1_einval", "invalid flags -> EINVAL"),
    "signalfd4": ("B13-signal", "signalfd4_einval", "invalid mask/fd -> EINVAL/EBADF"),
    "timerfd_create": ("B19-stubs", "timerfd_create_stub_semantics", "Starry dummy fd vs Linux"),
    "fanotify_init": ("B19-stubs", "fanotify_init_stub_semantics", "dummy fd"),
    "inotify_init1": ("B19-stubs", "inotify_init1_stub_semantics", "dummy fd"),
    "userfaultfd": ("B19-stubs", "userfaultfd_stub_semantics", "dummy fd"),
    "perf_event_open": ("B19-stubs", "perf_event_open_stub_semantics", "dummy fd"),
    "io_uring_setup": ("B19-stubs", "io_uring_setup_stub_semantics", "dummy fd"),
    "bpf": ("B19-stubs", "bpf_stub_semantics", "dummy fd"),
    "fsopen": ("B19-stubs", "fsopen_stub_semantics", "dummy fd"),
    "fspick": ("B19-stubs", "fspick_stub_semantics", "dummy fd"),
    "open_tree": ("B19-stubs", "open_tree_stub_semantics", "dummy fd"),
    "memfd_secret": ("B19-stubs", "memfd_secret_stub_semantics", "dummy fd"),
    "timer_create": ("B19-stubs", "timer_create_noop_semantics", "Ok(0) vs Linux timer_create"),
    "timer_gettime": ("B19-stubs", "timer_gettime_noop_semantics", "Ok(0) stub"),
    "timer_settime": ("B19-stubs", "timer_settime_noop_semantics", "Ok(0) stub"),
}

BATCH_BY_SECTION: dict[str, str] = {
    "fs ctl": "B01-fs-ctl",
    "file ops": "B02-file-ops",
    "fd ops": "B03-fd-ops",
    "io": "B04-io",
    "io mpx": "B05-io-mpx",
    "mm": "B06-mm",
    "fs stat": "B07-fs-stat",
    "fs mount": "B08-vfs-special",
    "pipe": "B08-vfs-special",
    "event": "B08-vfs-special",
    "pidfd": "B08-vfs-special",
    "memfd": "B08-vfs-special",
    "task ops": "B09-task-ops",
    "task management": "B10-task-mgmt",
    "task sched": "B11-task-sched",
    "task info": "B12-task-info",
    "signal": "B13-signal",
    "signal file descriptors": "B13-signal",
    "sys": "B14-sys",
    "time": "B15-time",
    "sync": "B16-sync",
    "msg": "B17-ipc",
    "shm": "B17-ipc",
    "net": "B18-net",
    "dummy fds": "B19-stubs",
}


def default_batch(section: str, handler: str) -> str:
    if handler == "sys_dummy_fd" or handler == "Ok(0)":
        return "B19-stubs"
    return BATCH_BY_SECTION.get(section, "B14-sys")


def planned_row(
    syscall: str, section: str, handler: str
) -> tuple[str, str, str, str]:
    """Returns (batch_id, planned_contract_probe, suggested_case, probe_pattern)."""
    if syscall in SPECIAL_PROBE:
        bid, probe, case = SPECIAL_PROBE[syscall]
        return bid, probe, case, "special"

    bid = default_batch(section, handler)

    if handler == "sys_dummy_fd":
        return bid, f"{syscall}_stub_fd_semantics", "dummy fd vs Linux; likely divergent matrix row later", "stub_fd"
    if handler == "Ok(0)":
        return bid, f"{syscall}_noop_ret0_semantics", "Starry returns Ok(0); vs Linux real timer API", "noop_stub"

    if syscall in FD_BADFD_FIRST:
        return bid, f"{syscall}_badfd", "fd=-1 -> EBADF (errno=9)", "errno_badfd"
    if syscall in PATH_ENOENT:
        return bid, f"{syscall}_enoent", "missing abs path -> ENOENT (errno=2)", "errno_enoent_path"
    if syscall in AT_PATH_ENOENT:
        return bid, f"{syscall}_enoent", "AT_FDCWD + missing path -> ENOENT", "errno_enoent_at"
    if syscall in NULL_STRUCT_EFAULT:
        return bid, f"{syscall}_null_ptr_efault", "NULL result struct -> EFAULT (errno=14)", "errno_efault_null"

    # sched_* / prctl / cap* / set*uid — minimal EINVAL or EPERM corners vary; generic placeholder.
    return bid, f"{syscall}_linux_contract_p1", "pick minimal errno oracle case per man 2", "tbd_errno"


def main() -> int:
    if yaml is None:
        print("PyYAML required", file=sys.stderr)
        return 2

    root = Path(__file__).resolve().parent.parent
    dispatch_path = root / "docs" / "starryos-syscall-dispatch.json"
    mod_path = root / "os" / "StarryOS" / "kernel" / "src" / "syscall" / "mod.rs"
    matrix_path = root / "docs" / "starryos-syscall-compat-matrix.yaml"
    rollout_path = root / "docs" / "starryos-syscall-probe-rollout.yaml"

    payload = json.loads(dispatch_path.read_text(encoding="utf-8"))
    rows: list[dict] = payload.get("syscalls") or []
    mod_text = mod_path.read_text(encoding="utf-8")
    handlers = handlers_by_syscall(mod_text)

    matrix_text = matrix_path.read_text(encoding="utf-8")
    m_hdr = re.search(r"(?ms)^entries:\s*$(?:\r?\n)", matrix_text)
    header_prefix = matrix_text[: m_hdr.start()] if m_hdr else ""
    data = yaml.safe_load(matrix_text)
    entries: list[dict] = list(data.get("entries") or [])

    by_name: dict[str, dict] = {}
    for e in entries:
        if isinstance(e, dict) and "syscall" in e:
            by_name[str(e["syscall"])] = e

    rollout_batches: dict[str, list[dict]] = {}

    for r in rows:
        syscall = str(r["syscall"])
        section = str(r.get("section_comment") or "")
        handler = handlers.get(syscall, "")

        if syscall in DONE_PARTIAL:
            p = str(by_name.get(syscall, {}).get("parity") or "")
            if p not in ("partial", "aligned"):
                print(f"warn: {syscall} in DONE_PARTIAL but matrix parity is {p!r}", file=sys.stderr)
            continue

        if syscall in by_name:
            existing = by_name[syscall]
            p = str(existing.get("parity") or "")
            if p in ("partial", "aligned", "divergent"):
                continue

        bid, probe, case, pattern = planned_row(syscall, section, handler)
        new_entry = {
            "syscall": syscall,
            "parity": "not_applicable",
            "planned_contract_probe": probe,
            "notes": [
                f"rollout_batch={bid}",
                f"suggested_case={case}",
                f"probe_pattern={pattern}",
                f"handler={handler}",
                f"section={section}",
                "Promote to parity partial after contract/*.c + expected exist; then run check_compat_matrix.py",
            ],
        }
        if syscall in by_name:
            idx = next(i for i, x in enumerate(entries) if x.get("syscall") == syscall)
            entries[idx] = new_entry
            by_name[syscall] = new_entry
        else:
            entries.append(new_entry)
            by_name[syscall] = new_entry

        rollout_batches.setdefault(bid, []).append(
            {
                "syscall": syscall,
                "planned_contract_probe": probe,
                "suggested_case": case,
                "probe_pattern": pattern,
                "handler": handler,
                "section": section,
            }
        )

    # Stable order: existing partial rows first (keep file order), then append sorted by batch, syscall
    preserved: list[dict] = []
    tail: list[dict] = []
    seen_tail = set()
    for e in entries:
        if not isinstance(e, dict):
            continue
        s = str(e.get("syscall") or "")
        p = str(e.get("parity") or "")
        if p in ("partial", "aligned", "divergent") or s == "io_zero_rw":
            preserved.append(e)
        else:
            tail.append(e)
            seen_tail.add(s)

    tail.sort(
        key=lambda x: (
            str(x.get("notes", ["rollout_batch=B99"])[0]),
            str(x.get("syscall")),
        )
    )
    merged = preserved + tail

    data["entries"] = merged
    entries_yaml = yaml.dump(
        {"entries": merged},
        allow_unicode=True,
        default_flow_style=False,
        sort_keys=False,
        width=120,
    )
    matrix_path.write_text(header_prefix + entries_yaml, encoding="utf-8")
    print(f"Wrote {matrix_path} ({len(merged)} entries)")

    batch_list = sorted(rollout_batches.items(), key=lambda x: x[0])
    rollout_doc = {
        "schema_version": 1,
        "description": "Planned Linux syscall contract probes (batches). Matrix uses parity not_applicable until promoted.",
        "done_partial_reference": sorted(DONE_PARTIAL),
        "batch_order_hint": [b for b, _ in batch_list],
        "batches": [
            {
                "id": bid,
                "title": bid.replace("-", " ").title(),
                "syscall_count": len(items),
                "syscalls": sorted(items, key=lambda x: x["syscall"]),
            }
            for bid, items in batch_list
        ],
    }
    rollout_path.write_text(
        yaml.dump(
            rollout_doc,
            allow_unicode=True,
            default_flow_style=False,
            sort_keys=False,
            width=100,
        ),
        encoding="utf-8",
    )
    print(f"Wrote {rollout_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
