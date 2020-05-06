#include <bdev/aio/bdev_aio.h>
#include <bdev/crypto/vbdev_crypto.h>
#include <bdev/iscsi/bdev_iscsi.h>
#include <bdev/lvol/vbdev_lvol.h>
#include <bdev/malloc/bdev_malloc.h>
#include <bdev/nvme/bdev_nvme.h>
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
#include <spdk/iscsi_spec.h>
#include <spdk/jsonrpc.h>
#include <spdk/log.h>
#include <spdk/lvol.h>
#include <spdk/nbd.h>
#include <spdk/nvme.h>
#include <spdk/nvme_spec.h>
#include <spdk/nvmf.h>
#include <spdk/pci_ids.h>
#include <spdk/queue.h>
#include <spdk/queue_extras.h>
#include <spdk/rpc.h>
#include <spdk/scsi.h>
#include <spdk/scsi_spec.h>
#include <spdk/thread.h>
#include <spdk/uuid.h>
#include <spdk/version.h>
#include <spdk_internal/event.h>
#include <spdk_internal/lvolstore.h>

typedef void maya_logger(int level, const char *file, const int line,
    const char *func, const char *buf, const int len);
// pointer is set from within rust to point to our logging trampoline
maya_logger *logfn = NULL;
