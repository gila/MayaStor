use mayastor::{
    core::{
        mayastor_env_stop,
        Bdev,
        BdevHandle,
        MayastorCliArgs,
        MayastorEnvironment,
        Reactors,
    },
    logger,
    nexus_uri::bdev_create,
};
use std::convert::TryFrom;
mayastor::CPS_INIT!();
use mayastor::core::{Descriptor, DmaBuf};
use spdk_sys::*;
use std::{os::raw::c_void, ptr::NonNull};
extern "C" {
    pub fn bdev_io_init(
        io: *mut spdk_bdev_io,
        bdev: *mut spdk_bdev,
        cb_arg: *mut ::std::os::raw::c_void,
        cb: spdk_bdev_io_completion_cb,
    );
}
extern "C" {
    pub fn bdev_io_submit(io: *mut spdk_bdev_io);
}
extern "C" {
    pub fn bdev_channel_get_io(
        channel: *mut spdk_bdev_channel,
    ) -> *mut spdk_bdev_io;
}

extern "C" fn cas_completion(
    io: *mut spdk_bdev_io,
    success: bool,
    arg: *mut c_void,
) {
    if success {
        println!("wholly shit batman! it worked!");
        unsafe {
            return spdk_bdev_io_complete(io, SPDK_BDEV_IO_STATUS_SUCCESS);
        }
    }

    unsafe {
        spdk_bdev_io_complete(io, SPDK_BDEV_IO_STATUS_FAILED);
    }
}

async fn start2() {
    unsafe {
        signal_hook::register(signal_hook::SIGINT, || {
            println!("impatient huh?!");
            std::process::exit(1);
        })
        .unwrap();

        let b = bdev_create("malloc:///malloc0?size_mb=10")
            .await
            .map(|name| Bdev::lookup_by_name(&name).unwrap())
            .unwrap();

        let desc = b.open(false).unwrap();

        let buf = DmaBuf::new(512, 9).unwrap();
        let channel = desc.get_channel().unwrap();
        dbg!(&channel);
        let bdev_channel: *mut c_void = (channel.as_ptr() as *mut u8)
            .add(::std::mem::size_of::<spdk_io_channel>() as usize)
            as *mut _;

        let spdk_bdev_channel = bdev_io_channel_get_ctx(channel.as_ptr());

        std::mem::forget(channel);
        let mut io = bdev_channel_get_io(spdk_bdev_channel as *mut _);
        (*io).u.bdev.iovs = &mut (*io).iov;
        // *(*io).u.bdev.iovs[0].iov_base = *buf;
        // *(*io).u.bdev.iovs[0].iov_len = b.block_len() as u64;
        (*io).u.bdev.iovcnt = 1;
        (*io).u.bdev.md_buf = std::ptr::null_mut();
        (*io).u.bdev.num_blocks = 1;
        (*io).u.bdev.offset_blocks = 0;
        (*io).internal.ch = spdk_bdev_channel as *mut _;
        (*io).internal.desc = desc.as_ptr();
        (*io).type_ = SPDK_BDEV_IO_TYPE_READ as u8;

        std::mem::forget(buf);
        std::mem::forget(desc);
        bdev_io_init(
            io,
            b.as_ptr(),
            std::ptr::null_mut(),
            Some(cas_completion),
        );

        bdev_io_submit(io);
    }
}

async fn start() {
    unsafe {
        signal_hook::register(signal_hook::SIGINT, || {
            println!("impatient huh?!");
            std::process::exit(1);
        })
    }
    .unwrap();

    let b = bdev_create("malloc:///malloc0?size_mb=10")
        .await
        .map(|name| Bdev::lookup_by_name(&name).unwrap())
        .unwrap();

    let hdl = b.open(true).and_then(BdevHandle::try_from).unwrap();

    println!("Bdev {:?} opened", hdl);
    let mut buf = hdl.dma_malloc(hdl.get_bdev().block_len() as usize).unwrap();
    let start = std::time::Instant::now();
    for l in 0 .. hdl.get_bdev().num_blocks() {
        hdl.read_at(l * 512, &mut buf).await.unwrap();
    }

    println!(
        "done reading all blocks {}, in sequential order in  {} seconds",
        hdl.get_bdev().num_blocks(),
        start.elapsed().as_secs()
    );

    mayastor_env_stop(0)
}

fn main() {
    let mut args = MayastorCliArgs::default();
    logger::init("DEBUG");
    args.reactor_mask = "0x2".to_string();
    args.log_components = vec!["bdev_malloc".to_string()];
    args.grpc_endpoint = Some("0.0.0.0".to_string());

    let ms = MayastorEnvironment::new(args);
    ms.start(|| Reactors::master().send_future(start2()))
        .unwrap();
}
