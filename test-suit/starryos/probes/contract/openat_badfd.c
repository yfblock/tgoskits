/* Hand-written contract probe: openat(2) with invalid dirfd -> EBADF. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>

int main(void)
{
	errno = 0;
	int r = openat(-1, "probe_relative_name", O_RDONLY | O_NOCTTY);
	int e = errno;
	dprintf(1, "CASE openat.bad_dirfd ret=%d errno=%d note=handwritten\n", r, e);
	return 0;
}
