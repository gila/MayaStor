#include <bdev/aio/bdev_aio.h>
#include <bdev/crypto/vbdev_crypto.h>
#include <bdev/error/vbdev_error.h>
#include <bdev/iscsi/bdev_iscsi.h>
#include <bdev/lvol/vbdev_lvol.h>
#include <bdev/nvme/bdev_nvme.h>
#include <bdev/malloc/bdev_malloc.h>
#include <bdev/uring/bdev_uring.h>
#include <iscsi/init_grp.h>
#include <iscsi/iscsi.h>
#include <iscsi/portal_grp.h>
#include <iscsi/tgt_node.h>
#include <nbd/nbd_internal.h>
#include <spdk/bdev.h>
#include <spdk/bdev_module.h>
#include <spdk/conf.h>
#include <spdk/cpuset.h>
#include <spdk/env.h>
#include <spdk/env_dpdk.h>
#include <spdk/event.h>
#include <spdk/jsonrpc.h>
#include <spdk/log.h>
#include <spdk/lvol.h>
#include <spdk/nbd.h>
#include <spdk/nvme.h>
#include <spdk/nvmf.h>
#include <nvmf/nvmf_internal.h>
#include <spdk/rpc.h>
#include <spdk/scsi.h>
#include <spdk/thread.h>
#include <spdk/uuid.h>
#include <spdk/version.h>
#include <spdk_internal/event.h>
#include <spdk_internal/thread.h>
#include <spdk_internal/lvolstore.h>

#include "logwrapper.h"

struct spdk_bdev_io *
_bdev_channel_get_io(struct spdk_bdev_channel *channel) {
	bdev_channel_get_io(channel);
}

void
_bdev_io_init(struct spdk_bdev_io *io, struct spdk_bdev* bdev, void *cb_arg, spdk_bdev_io_completion_cb cb) {
	bdev_io_init(io, bdev, cb_arg, cb);
}

void
_bdev_io_submit(struct spdk_bdev_io *io) {
	bdev_io_submit(io);
}
