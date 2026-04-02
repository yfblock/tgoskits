"""C statement (inside main, after errno=0) for fd=-1 -> EBADF probes. Value: (includes, stmt, use_long_r)."""

from __future__ import annotations

BADFD_STMT: dict[str, tuple[list[str], str, bool]] = {
    "accept": (["errno.h", "stdio.h", "sys/socket.h"], "int r = accept(-1, NULL, NULL);", False),
    "accept4": (["errno.h", "stdio.h", "sys/socket.h"], "int r = accept4(-1, NULL, NULL, 0);", False),
    "bind": (["errno.h", "stdio.h", "sys/socket.h"], "int r = bind(-1, NULL, 0);", False),
    "connect": (["errno.h", "stdio.h", "sys/socket.h"], "int r = connect(-1, NULL, 0);", False),
    "copy_file_range": (
        ["errno.h", "stdio.h", "unistd.h", "sys/syscall.h"],
        "long r = syscall(SYS_copy_file_range, -1, NULL, -1, NULL, 0, 0);",
        True,
    ),
    "dup2": (["errno.h", "stdio.h", "unistd.h"], "int r = dup2(-1, 2);", False),
    "dup3": (["errno.h", "stdio.h", "unistd.h"], "int r = dup3(-1, 2, 0);", False),
    "epoll_ctl": (
        ["errno.h", "stdio.h", "string.h", "sys/epoll.h", "unistd.h"],
        """struct epoll_event ev;
	memset(&ev, 0, sizeof(ev));
	int epfd = epoll_create1(0);
	int r = epoll_ctl(epfd, EPOLL_CTL_ADD, -1, &ev);
	close(epfd);""",
        False,
    ),
    "epoll_pwait": (
        ["errno.h", "stdio.h", "string.h", "sys/epoll.h", "signal.h", "unistd.h"],
        """struct epoll_event ev[1];
	memset(ev, 0, sizeof(ev));
	int fd = epoll_create1(0);
	close(fd);
	int r = epoll_pwait(fd, ev, 1, 0, NULL);""",
        False,
    ),
    "epoll_pwait2": (
        ["errno.h", "stdio.h", "string.h", "sys/epoll.h", "sys/syscall.h", "time.h", "unistd.h"],
        """struct epoll_event ev[1];
	memset(ev, 0, sizeof(ev));
	int fd = epoll_create1(0);
	close(fd);
	struct timespec ts = {0, 0};
	long r = syscall(441, (long)fd, ev, 1, &ts, NULL);""",
        True,
    ),
    "fadvise64": (
        ["errno.h", "stdio.h", "sys/syscall.h", "unistd.h"],
        "long r = syscall(SYS_fadvise64, -1, 0, 0, 0);",
        True,
    ),
    "fallocate": (["errno.h", "stdio.h", "fcntl.h"], "int r = fallocate(-1, 0, 0, 0);", False),
    "fchmod": (["errno.h", "stdio.h", "sys/stat.h"], "int r = fchmod(-1, 0777);", False),
    "fchown": (["errno.h", "stdio.h", "unistd.h"], "int r = fchown(-1, (uid_t)-1, (gid_t)-1);", False),
    "fdatasync": (["errno.h", "stdio.h", "unistd.h"], "int r = fdatasync(-1);", False),
    "flock": (["errno.h", "stdio.h", "sys/file.h"], "int r = flock(-1, LOCK_EX);", False),
    "fsync": (["errno.h", "stdio.h", "unistd.h"], "int r = fsync(-1);", False),
    "fstat": (["errno.h", "stdio.h", "sys/stat.h"], "struct stat st; int r = fstat(-1, &st);", False),
    "fstatfs": (["errno.h", "stdio.h", "sys/statfs.h"], "struct statfs sf; int r = fstatfs(-1, &sf);", False),
    "ftruncate": (["errno.h", "stdio.h", "unistd.h"], "int r = ftruncate(-1, 0);", False),
    "getpeername": (["errno.h", "stdio.h", "sys/socket.h"], "int r = getpeername(-1, NULL, NULL);", False),
    "getsockname": (["errno.h", "stdio.h", "sys/socket.h"], "int r = getsockname(-1, NULL, NULL);", False),
    "getsockopt": (
        ["errno.h", "stdio.h", "sys/socket.h"],
        "int v; socklen_t l = sizeof(v); int r = getsockopt(-1, SOL_SOCKET, SO_TYPE, &v, &l);",
        False,
    ),
    "listen": (["errno.h", "stdio.h", "sys/socket.h"], "int r = listen(-1, 1);", False),
    "pidfd_getfd": (
        ["errno.h", "stdio.h", "sys/syscall.h", "unistd.h"],
        "int r = (int)syscall(SYS_pidfd_getfd, -1, -1, 0);",
        False,
    ),
    "pidfd_send_signal": (
        ["errno.h", "stdio.h", "signal.h", "sys/syscall.h", "unistd.h"],
        "int r = (int)syscall(SYS_pidfd_send_signal, -1, SIGKILL, NULL, 0);",
        False,
    ),
    "pread64": (["errno.h", "stdio.h", "unistd.h"], "char b; ssize_t r = pread(-1, &b, 1, 0);", False),
    "preadv": (
        ["errno.h", "stdio.h", "sys/uio.h", "unistd.h"],
        "char b; struct iovec iov = { &b, 1 }; ssize_t r = preadv(-1, &iov, 1, 0);",
        False,
    ),
    "preadv2": (
        ["errno.h", "stdio.h", "sys/uio.h", "sys/syscall.h", "unistd.h"],
        "char b; struct iovec iov = { &b, 1 }; long r = syscall(SYS_preadv2, -1, &iov, 1, 0, 0);",
        True,
    ),
    "pwrite64": (["errno.h", "stdio.h", "unistd.h"], "char b = 0; ssize_t r = pwrite(-1, &b, 1, 0);", False),
    "pwritev": (
        ["errno.h", "stdio.h", "sys/uio.h", "unistd.h"],
        "char b = 0; struct iovec iov = { &b, 1 }; ssize_t r = pwritev(-1, &iov, 1, 0);",
        False,
    ),
    "pwritev2": (
        ["errno.h", "stdio.h", "sys/uio.h", "sys/syscall.h", "unistd.h"],
        "char b = 0; struct iovec iov = { &b, 1 }; long r = syscall(SYS_pwritev2, -1, &iov, 1, 0, 0);",
        True,
    ),
    "readv": (
        ["errno.h", "stdio.h", "sys/uio.h", "unistd.h"],
        "char b; struct iovec iov = { &b, 1 }; ssize_t r = readv(-1, &iov, 1);",
        False,
    ),
    "writev": (
        ["errno.h", "stdio.h", "sys/uio.h", "unistd.h"],
        "char b = 0; struct iovec iov = { &b, 1 }; ssize_t r = writev(-1, &iov, 1);",
        False,
    ),
    "recvfrom": (
        ["errno.h", "stdio.h", "sys/socket.h"],
        "char b; ssize_t r = recvfrom(-1, &b, 1, 0, NULL, NULL);",
        False,
    ),
    "recvmsg": (["errno.h", "stdio.h", "sys/socket.h"], "ssize_t r = recvmsg(-1, NULL, 0);", False),
    "sendfile": (["errno.h", "stdio.h", "sys/sendfile.h"], "ssize_t r = sendfile(-1, -1, NULL, 0);", False),
    "sendmsg": (["errno.h", "stdio.h", "sys/socket.h"], "ssize_t r = sendmsg(-1, NULL, 0);", False),
    "sendto": (
        ["errno.h", "stdio.h", "sys/socket.h"],
        "char b = 0; ssize_t r = sendto(-1, &b, 1, 0, NULL, 0);",
        False,
    ),
    "setsockopt": (
        ["errno.h", "stdio.h", "sys/socket.h"],
        "int v = 1; int r = setsockopt(-1, SOL_SOCKET, SO_REUSEADDR, &v, sizeof(v));",
        False,
    ),
    "shutdown": (["errno.h", "stdio.h", "sys/socket.h"], "int r = shutdown(-1, SHUT_RDWR);", False),
    "splice": (
        ["errno.h", "stdio.h", "fcntl.h", "unistd.h"],
        "ssize_t r = splice(0, NULL, -1, NULL, 1, 0);",
        False,
    ),
}
