#include <stddef.h>
#include <stdarg.h>
#include <spdk/log.h>
#include <spdk/thread.h>

typedef void maya_logger(int level, const char *file, const int line,
    const char *func, const char *buf, const int len);

// pointer is set from within rust to point to our logging trampoline
maya_logger *logfn = NULL;

void*
bdev_io_channel_get_ctx(struct spdk_io_channel *ch);


