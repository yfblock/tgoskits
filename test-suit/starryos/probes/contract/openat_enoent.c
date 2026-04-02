/* Hand-written contract probe: openat(2) nonexistent absolute path -> ENOENT. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>

#ifndef AT_FDCWD
#define AT_FDCWD (-100)
#endif

int main(void)
{
	errno = 0;
	int r = openat(AT_FDCWD, "/__starryos_probe_openat_enoent__/not_there", O_RDONLY | O_NOCTTY);
	int e = errno;
	dprintf(1, "CASE openat.enoent ret=%d errno=%d note=handwritten\n", r, e);
	return 0;
}
