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
use mayastor::core::{Descriptor, DmaBuf, Reactor};
use spdk_sys::*;
use std::{
    os::raw::c_void,
    ptr::{slice_from_raw_parts_mut, NonNull},
};

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
    }

    unsafe {
        spdk_bdev_free_io(io);
    }

    mayastor_env_stop(0);
}

async fn start4() {
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
        let channel = desc.get_channel().unwrap();
        let buf = DmaBuf::new(512, 9).unwrap();

        let mut bio = NonNull::new(bdev_get_bio(
            channel.as_ptr(),
            desc.as_ptr(),
            std::ptr::null_mut(),
        ))
        .unwrap();

        bio.as_mut().u.bdev.iovs = &mut bio.as_mut().iov;
        bio.as_mut().u.bdev.iovcnt = 1;
        bio.as_mut().u.bdev.num_blocks = 1;
        bio.as_mut().type_ = SPDK_BDEV_IO_TYPE_READ as u8;

        let mut iov =
            std::slice::from_raw_parts_mut(bio.as_mut().u.bdev.iovs, 1);
        iov[0].iov_base = *buf;
        iov[0].iov_len = 512;
        //NOTE: internal.ch set in get_bio()
        bio.as_mut().internal.desc = desc.as_ptr();

        bdev_io_init(
            bio.as_ptr(),
            b.as_ptr(),
            std::ptr::null_mut(),
            Some(cas_completion),
        );
        std::mem::forget(desc);
        std::mem::forget(channel);
        std::mem::forget(buf);
        bdev_io_submit(bio.as_ptr());
    }
}
async fn start3() {
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
        let channel = desc.get_channel().unwrap();
        let buf = DmaBuf::new(512, 9).unwrap();

        let bio =
            bdev_get_bio(channel.as_ptr(), desc.as_ptr(), std::ptr::null_mut());

        (*bio).u.bdev.iovs = &mut (*bio).iov;
        (*bio).u.bdev.iovcnt = 1;
        (*bio).u.bdev.num_blocks = 1;
        (*bio).type_ = SPDK_BDEV_IO_TYPE_READ as u8;

        let mut iov = std::slice::from_raw_parts_mut((*bio).u.bdev.iovs, 1);
        iov[0].iov_base = *buf;
        iov[0].iov_len = 512;
        //NOTE: internal.ch set in get_bio()
        (*bio).internal.desc = desc.as_ptr();

        bdev_io_init(
            bio,
            b.as_ptr(),
            std::ptr::null_mut(),
            Some(cas_completion),
        );
        std::mem::forget(desc);
        std::mem::forget(channel);
        std::mem::forget(buf);
        bdev_io_submit(bio);

        //
        // let desc = b.open(true).unwrap();
        //
        // let buf = DmaBuf::new(512, b.alignment()).unwrap();
        // let channel = desc.get_channel().unwrap();
        //
        // spdk_bdev_read(
        //     desc.as_ptr(),
        //     channel.as_ptr(),
        //     *buf,
        //     0,
        //     buf.len() as u64,
        //     Some(cas_completion),
        //     std::ptr::null_mut(),
        // );
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
        let channel = desc.get_channel().unwrap();
        let buf = DmaBuf::new(512, 9).unwrap();
        let bio = bdev_get_bio(channel.as_ptr(), desc.as_ptr(), *buf);

        let mut iov = std::slice::from_raw_parts_mut((*bio).u.bdev.iovs, 1);
        iov[0].iov_len = 512;
        (*bio).u.bdev.num_blocks = 1;
        (*bio).type_ = SPDK_BDEV_IO_TYPE_READ as u8;

        bdev_io_init(
            bio,
            b.as_ptr(),
            std::ptr::null_mut(),
            Some(cas_completion),
        );
        bdev_io_submit(bio);

        //
        // let desc = b.open(true).unwrap();
        //
        // let buf = DmaBuf::new(512, b.alignment()).unwrap();
        // let channel = desc.get_channel().unwrap();
        //
        // spdk_bdev_read(
        //     desc.as_ptr(),
        //     channel.as_ptr(),
        //     *buf,
        //     0,
        //     buf.len() as u64,
        //     Some(cas_completion),
        //     std::ptr::null_mut(),
        // );

        // std::mem::forget(desc);
        // std::mem::forget(channel);
        // std::mem::forget(buf);
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

    let b = bdev_create("malloc:///malloc0?size_mb=1")
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
    ms.start(|| {
        Reactor::block_on(async {
            start4().await;
        })
        .unwrap();
    })
    .unwrap();
}
