"""Full C sources keyed by planned_contract_probe (oracle via materialize_syscall_batch.run_oracle)."""

from __future__ import annotations

EXTRA_C: dict[str, str] = {
    "epoll_create1_einval": r"""
#include <errno.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>
int main(void)
{
	errno = 0;
	long r = syscall(SYS_epoll_create1, -1);
	int e = errno;
	dprintf(1, "CASE epoll_create1.einval ret=%ld errno=%d note=handwritten\n", r, e);
	return 0;
}
""",
    "poll_linux_contract_p1": r"""
#include <errno.h>
#include <poll.h>
#include <stdio.h>
int main(void)
{
	errno = 0;
	int r = poll(NULL, -1, 0);
	int e = errno;
	dprintf(1, "CASE poll.einval ret=%d errno=%d note=handwritten\n", r, e);
	return 0;
}
""",
    "select_linux_contract_p1": r"""
#include <errno.h>
#include <stdio.h>
#include <sys/select.h>
int main(void)
{
	errno = 0;
	int r = select(-1, NULL, NULL, NULL, NULL);
	int e = errno;
	dprintf(1, "CASE select.einval ret=%d errno=%d note=handwritten\n", r, e);
	return 0;
}
""",
    "pselect6_linux_contract_p1": r"""
#include <errno.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>
int main(void)
{
	errno = 0;
	long r = syscall(SYS_pselect6, -1, NULL, NULL, NULL, NULL, NULL);
	int e = errno;
	dprintf(1, "CASE pselect6.einval ret=%ld errno=%d note=handwritten\n", r, e);
	return 0;
}
""",
    "brk_increment_smoke": r"""
#include <errno.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>
int main(void)
{
	void *b = (void *)syscall(SYS_brk, (void *)0);
	errno = 0;
	void *b2 = (void *)syscall(SYS_brk, b);
	int e = errno;
	int r = (b2 == b) ? 0 : -1;
	dprintf(1, "CASE brk_increment.smoke ret=%d errno=%d note=handwritten\n", r, e);
	return 0;
}
""",
    "mmap_nonanon_badfd": r"""
#include <errno.h>
#include <stdio.h>
#include <sys/mman.h>
int main(void)
{
	errno = 0;
	void *p = mmap(NULL, 4096, PROT_READ, MAP_PRIVATE, -1, 0);
	long r = (long)(unsigned long)p;
	int e = errno;
	dprintf(1, "CASE mmap.nonanon_badfd ret=%ld errno=%d note=handwritten\n", r, e);
	return 0;
}
""",
    "munmap_einval": r"""
#include <errno.h>
#include <stdio.h>
#include <sys/mman.h>
int main(void)
{
	errno = 0;
	int r = munmap((void *)1, 0);
	int e = errno;
	dprintf(1, "CASE munmap.einval ret=%d errno=%d note=handwritten\n", r, e);
	return 0;
}
""",
    "mprotect_einval": r"""
#include <errno.h>
#include <stdio.h>
#include <sys/mman.h>
int main(void)
{
	void *p = mmap(NULL, 4096, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	errno = 0;
	int r = mprotect(p, 4096, PROT_READ | PROT_GROWSUP);
	int e = errno;
	dprintf(1, "CASE mprotect.einval ret=%d errno=%d note=handwritten\n", r, e);
	munmap(p, 4096);
	return 0;
}
""",
    "mincore_efault": r"""
#include <errno.h>
#include <stdio.h>
#include <sys/mman.h>
int main(void)
{
	void *p = mmap(NULL, 4096, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	errno = 0;
	int r = mincore(p, 4096, (unsigned char *)(void *)1);
	int e = errno;
	dprintf(1, "CASE mincore.efault ret=%d errno=%d note=handwritten\n", r, e);
	munmap(p, 4096);
	return 0;
}
""",
    "mremap_einval": r"""
#include <errno.h>
#include <stdio.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>
#ifndef SYS_mremap
#define SYS_mremap 216
#endif
#ifndef MREMAP_MAYMOVE
#define MREMAP_MAYMOVE 1
#endif
#ifndef MREMAP_FIXED
#define MREMAP_FIXED 2
#endif
int main(void)
{
	void *p = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	errno = 0;
	long r = (long)syscall(SYS_mremap, p, 4096UL, 4096UL, MREMAP_FIXED | MREMAP_MAYMOVE, (void *)1);
	int e = errno;
	dprintf(1, "CASE mremap.einval ret=%ld errno=%d note=handwritten\n", r, e);
	munmap(p, 4096);
	return 0;
}
""",
    "madvise_einval": r"""
#include <errno.h>
#include <stdio.h>
#include <sys/mman.h>
int main(void)
{
	errno = 0;
	int r = madvise((void *)1, 4096, MADV_NORMAL);
	int e = errno;
	dprintf(1, "CASE madvise.einval ret=%d errno=%d note=handwritten\n", r, e);
	return 0;
}
""",
    "msync_einval": r"""
#include <errno.h>
#include <stdio.h>
#include <sys/mman.h>
int main(void)
{
	errno = 0;
	int r = msync((void *)1, 0, MS_SYNC);
	int e = errno;
	dprintf(1, "CASE msync.einval ret=%d errno=%d note=handwritten\n", r, e);
	return 0;
}
""",
    "mlock_enomem": r"""
#include <errno.h>
#include <stdio.h>
#include <sys/mman.h>
int main(void)
{
	errno = 0;
	int r = mlock((void *)0x10000, 4096);
	int e = errno;
	dprintf(1, "CASE mlock.enomem ret=%d errno=%d note=handwritten\n", r, e);
	return 0;
}
""",
    "mlock2_einval": r"""
#include <errno.h>
#include <stdio.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>
#ifndef SYS_mlock2
#define SYS_mlock2 284
#endif
int main(void)
{
	void *p = mmap(NULL, 4096, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	errno = 0;
	long r = syscall(SYS_mlock2, p, 4096UL, (unsigned int)-1);
	int e = errno;
	dprintf(1, "CASE mlock2.einval ret=%ld errno=%d note=handwritten\n", r, e);
	munmap(p, 4096);
	return 0;
}
""",
}
