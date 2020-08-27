#include "logwrapper.h"

void
maya_log(int level, const char *file, const int line, const char *func,
    const char *format, va_list args)
{
    char buf[1024] = {0};
    int n_written = vsnprintf(buf, sizeof(buf), format, args);
    logfn(level, file, line, func, &buf[0], n_written);
}



void *
bdev_io_channel_get_ctx(struct spdk_io_channel *ch)
{
	return (uint8_t *)ch + sizeof(*ch);
}


struct spdk_bdev_io *
bdev_get_bio(struct spdk_io_channel *ch, struct spdk_bdev_desc *desc, void *buf)
{

	struct spdk_bdev *bdev = spdk_bdev_desc_get_bdev(desc);
	struct spdk_bdev_io *bdev_io;
	struct spdk_bdev_channel *channel = spdk_io_channel_get_ctx(ch);

	bdev_io = bdev_channel_get_io(channel);
	if (!bdev_io) {
		return NULL;
	}

	bdev_io->internal.ch = channel;
	bdev_io->internal.desc = desc;
	bdev_io->type = 0;
	bdev_io->u.bdev.iovs = &bdev_io->iov;
	bdev_io->u.bdev.iovs[0].iov_base = buf;
	bdev_io->u.bdev.iovs[0].iov_len = 0;
	bdev_io->u.bdev.iovcnt = 1;
	bdev_io->u.bdev.md_buf = NULL;
	bdev_io->u.bdev.num_blocks = 0;
	bdev_io->u.bdev.offset_blocks = 0;

	return bdev_io;

}



