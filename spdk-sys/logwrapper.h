#include <stddef.h>
#include <stdarg.h>
#include <spdk/log.h>
#include <spdk/bdev_module.h>
#include <spdk/bdev.h>

typedef void maya_logger(int level, const char *file, const int line,
    const char *func, const char *buf, const int len);

// pointer is set from within rust to point to our logging trampoline
maya_logger *logfn = NULL;

struct spdk_bdev_io* bdev_io_get_ctx(void *ctx);

