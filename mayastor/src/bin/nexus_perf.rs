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
use spdk_sys::{spdk_bdev_io, spdk_io_channel};
use std::{convert::TryFrom, ffi::c_void, sync::Arc};
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

    let mut submission = Vec::new();
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

async fn create_bdev() -> Arc<Descriptor> {
    let target = bdev_create("aio:///dev/nullb0")
        .await
        .map_err(|e| {
            error!("failed to create target bdev {}", e);
            mayastor_env_stop(-1);
        })
        .unwrap();

    Arc::new(
        Bdev::open_by_name(&target, false)
            .map_err(|e| {
                error!("failed to find bdev {}", e);
                mayastor_env_stop(-1);
            })
            .unwrap(),
    )
}

struct BdevIO {
    inner: *mut spdk_bdev_io,
}

impl BdevIO {
    fn prepare(d: Descriptor) -> Self {
        let bdev = d.get_bdev();
        let channel = d.get_channel().unwrap();
        let bdev_channel = unsafe {
            use std::mem::size_of;
            (channel.as_ptr()).add(size_of::<spdk_io_channel>()) as *mut c_void
        };

        //TODO bdev_valid_io_blocks
        let io = unsafe { spdk_sys::bdev_channel_get_io(bdev_channel) };
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
            let result = sequential_read(desc).await;
            dbg!(result);
            mayastor_env_stop(1);
        });
    })
    .unwrap();
}
