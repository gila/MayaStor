use futures::{
    task::{Context, Poll},
    Future,
};
use log::*;
use mayastor::{
    core::{
        mayastor_env_stop,
        Bdev,
        BdevHandle,
        Descriptor,
        DmaBuf,
        MayastorCliArgs,
        MayastorEnvironment,
        Reactors,
    },
    logger,
    nexus_uri::bdev_create,
};
use snafu::Error;
use spdk_sys::{
    bdev_io_submit,
    spdk_bdev,
    spdk_bdev_channel,
    spdk_bdev_desc_get_bdev,
    spdk_bdev_free_io,
    spdk_bdev_io,
    spdk_io_channel,
    SPDK_BDEV_IO_STATUS_PENDING,
    SPDK_BDEV_IO_TYPE_READ,
};
use std::{convert::TryFrom, ffi::c_void, ptr::NonNull, sync::Arc};
use tonic::codegen::Pin;
mayastor::CPS_INIT!();

pub struct Job {
    nr: u32,
    buf: DmaBuf,
}

async fn sequential_read(d: Arc<Descriptor>) -> Result<(), Box<dyn Error>> {
    let handle = BdevHandle::try_from(d)?;
    let num_blocks = handle.get_bdev().num_blocks();
    let blk_size = handle.get_bdev().block_len() as u64;

    let qd = 32;
    let mut buffers = Vec::new();
    for i in 0 .. 32 {
        buffers.push(handle.dma_malloc(blk_size as usize)?);
    }
    info!(
        "reading {} blocks from {} with blksize {} ({} MB)",
        num_blocks,
        handle.get_bdev().name(),
        blk_size,
        (num_blocks * blk_size) >> 20
    );
    todo!();
    let start = std::time::Instant::now();
    // //    for i in (0 .. num_blocks).step_by(32) {
    // for j in buffers.iter_mut() {
    //     submission.push(handle.read_at(0 + qd * blk_size, &mut j));
    // }
    // futures::future::join_all(submission);
    // //   }
    let seconds = start.elapsed().as_secs();
    info!(
        "reading completed in {} MB/s {} IOPS",
        ((blk_size * num_blocks) >> 20) / seconds,
        num_blocks / seconds
    );

    Ok(())
}

async fn read_one(d: &Descriptor) -> Result<(), ()> {
    let bio = BdevIO::prepare(d);
    bio.submit();
    let boom = bio.await;
    Ok(())
}

async fn create_bdev() -> Descriptor {
    let target = bdev_create("aio:///dev/nullb0")
        .await
        .map_err(|e| {
            error!("failed to create target bdev {}", e);
            mayastor_env_stop(-1);
        })
        .unwrap();

    Bdev::open_by_name(&target, false)
        .map_err(|e| {
            error!("failed to find bdev {}", e);
            mayastor_env_stop(-1);
        })
        .unwrap()
}
#[derive(Debug)]
struct BdevIO {
    inner: *mut spdk_bdev_io,
    buf: DmaBuf,
}

impl BdevIO {
    extern "C" fn io_completion(
        io: *mut spdk_bdev_io,
        success: bool,
        arg: *mut c_void,
    ) {
        println!("cb called!");
        dbg!("status: {}", success);
        unsafe {
            spdk_bdev_free_io(io);
        }
    }

    unsafe fn init(&mut self, bdev: *mut spdk_bdev) {
        // self.inner.as_mut().bdev = bdev.as_ptr();
        // self.inner.as_mut().internal.caller_ctx = std::ptr::null_mut();
        // self.inner.as_mut().internal.cb = Some(Self::io_completion);
        // self.inner.as_mut().internal.status = SPDK_BDEV_IO_STATUS_PENDING as
        // i8; self.inner.as_mut().internal.in_submit_request = false;
        // self.inner.as_mut().internal.buf = std::ptr::null_mut();
        // self.inner.as_mut().internal.io_submit_ch = std::ptr::null_mut();
        // self.inner.as_mut().internal.orig_iovs = std::ptr::null_mut();
        // self.inner.as_mut().internal.orig_md_buf = std::ptr::null_mut();
        // self.inner.as_mut().internal.error.nvme.cdw0 = 0;
        // self.inner.as_mut().num_retries = 0;
        // self.inner.as_mut().internal.get_aux_buf_cb = None;
        // self.inner.as_mut().internal.get_buf_cb = None;

        (*self.inner).bdev = bdev;
        (*self.inner).internal.caller_ctx = std::ptr::null_mut();
        (*self.inner).internal.cb = Some(Self::io_completion);
        (*self.inner).internal.status = SPDK_BDEV_IO_STATUS_PENDING as i8;
        (*self.inner).internal.in_submit_request = false;
        (*self.inner).internal.buf = std::ptr::null_mut();
        (*self.inner).internal.io_submit_ch = std::ptr::null_mut();
        (*self.inner).internal.orig_iovs = std::ptr::null_mut();
        (*self.inner).internal.orig_md_buf = std::ptr::null_mut();
        (*self.inner).internal.error.nvme.cdw0 = 0;
        (*self.inner).num_retries = 0;

        (*self.inner).internal.get_aux_buf_cb = None;
        (*self.inner).internal.get_buf_cb = None;
    }
    pub fn get_ctx(ch: *mut spdk_io_channel) -> *mut spdk_bdev_channel {
        unsafe {
            (ch as *mut u8).add(::std::mem::size_of::<spdk_io_channel>())
                as *mut spdk_bdev_channel
        }
    }

    fn prepare(d: &Descriptor) -> Self {
        let bdev = unsafe { spdk_bdev_desc_get_bdev(d.as_ptr()) };
        let channel = d.get_channel().unwrap().as_ptr();
        let bdev_channel = Self::get_ctx(channel);

        //TODO bdev_valid_io_blocks
        let io = unsafe {
            spdk_sys::bdev_channel_get_io(
                bdev_channel as *mut spdk_bdev_channel,
            )
        };

        dbg!(bdev_channel);
        unsafe {
            (*io).internal.ch = bdev_channel;
            (*io).internal.desc = d.as_ptr();
            (*io).type_ = 1;
            (*io).u.bdev.iovs = std::ptr::null_mut();
            (*io).u.bdev.md_buf = std::ptr::null_mut();
            (*io).u.bdev.num_blocks = 1;
            (*io).u.bdev.offset_blocks = 0;
        };

        unsafe {
            let mut bio = Self {
                inner: io,
                buf: DmaBuf::new(512, 9).unwrap(),
            };

            bio.init(bdev);
            bio
        }
    }

    pub fn submit(&self) {
        unsafe { bdev_io_submit(self.inner) };
    }
}

impl Future for BdevIO {
    type Output = bool;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        dbg!("poll!!");
        match unsafe { (*self.inner).internal.status as i32 } {
            SPDK_BDEV_IO_STATUS_PENDING => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }

            SPDK_BDEV_IO_STATUS_FAILED => Poll::Ready(false),
            SPDK_BDEV_IO_STATUS_SUCCES => Poll::Ready(true),

            _ => panic!("invalid state"),
        }
    }
}

fn main() {
    logger::init("INFO");
    let ms = MayastorEnvironment::new(MayastorCliArgs {
        config: None,
        grpc_endpoint: None,
        ..Default::default()
    });

    ms.start(|| {
        unsafe {
            signal_hook::register(signal_hook::SIGINT, || {
                std::process::exit(1);
            })
        }
        .unwrap();

        unsafe {
            signal_hook::register(signal_hook::SIGTERM, || {
                std::process::exit(1);
            })
        }
        .unwrap();
        Reactors::current().send_future(async {
            let desc = create_bdev().await;
            let result = read_one(&desc).await;
        });
    })
    .unwrap();
}
