#![feature(async_await)]
use mayastor::{mayastor_start, spdk_stop, syncshot};

use mayastor::{
    descriptor::{Descriptor, Descriptorun},
    nexus_uri::{nexus_parse_uri, BdevType},
};
use spdk_sys::{
    spdk_bdev_close,
    spdk_bdev_desc,
    spdk_bdev_free_io,
    spdk_bdev_io,
    spdk_bdev_read,
    spdk_bdev_write,
    spdk_io_channel,
    spdk_put_io_channel,
};
use std::{ffi::c_void, sync::mpsc::sync_channel};

static DISKNAME: &str = "/dev///nullb0";
static BDEVNAME: &str = "Malloc0";

#[test]
fn io_compare() {
    let _log = mayastor::spdklog::SpdkLog::new();
    let _l = _log.init();
    mayastor::CPS_INIT!();
    let args =
        vec!["test", "-m", "0x08", "-c", "/home/gila/MayaStor/test.conf"];

    mayastor_start("io-testing", args, || {
        mayastor::executor::spawn(start());
    });
}

async fn start() {
    //    match nexus_parse_uri(BDEVNAME) {
    //        Ok(BdevType::Aio(args)) => {
    //            let _ = args.create().await.expect("failed to create");
    //        }
    //        _ => {
    //            panic!("invalid test configuration");
    //        }
    //    };

    //read_some().await;
    unread_some().await;
    spdk_stop(0)
    //  cb_read_some();
}

async fn read_some() {
    let d = Descriptor::open(BDEVNAME, true);
    let d = d.unwrap();

    let mut buf1 = d.dma_zmalloc(512).expect("failed to allocate buffer");
    let start = std::time::Instant::now();
    for i in 0 .. 1024000 {
        let _ = d.write_at(i * 512, &mut buf1).await;
    }

    for i in 0 .. 1024000 {
        let _ = d.write_at(i * 512, &mut buf1).await;
    }
    for i in 0 .. 1024000 {
        let _ = d.write_at(i * 512, &mut buf1).await;
    }
    for i in 0 .. 1024000 {
        let _ = d.write_at(i * 512, &mut buf1).await;
    }

    for i in 0 .. 1024000 {
        let _ = d.write_at(i * 512, &mut buf1).await;
    }

    println!(
        "ARC({}), {:?}",
        d.get_bdev().num_blocks(),
        std::time::Instant::now().duration_since(start)
    );
}

async fn unread_some() {
    let d = Descriptorun::open(BDEVNAME, true);
    let d = d.unwrap();

    let mut buf1 = d.dma_zmalloc(512).expect("failed to allocate buffer");
    let start = std::time::Instant::now();

    for i in 0 .. 1024000 {
        d.read_at(i * 512, &mut buf1).await;
    }

    println!(
        "RC({}), {:?}",
        d.get_bdev().num_blocks(),
        std::time::Instant::now().duration_since(start)
    );
}

extern "C" fn io_complete(
    io: *mut spdk_bdev_io,
    success: bool,
    arg: *mut c_void,
) {
    assert_eq!(success, true);
    let mut ctx = unsafe { Box::from_raw(arg as *const _ as *mut IoCtx) };
    ctx.count += 1;

    unsafe {
        spdk_bdev_free_io(io);
    }

    if ctx.count == 1024000 {
        println!(
            "use CB {:?}",
            std::time::Instant::now().duration_since(ctx.start)
        );
        unsafe {
            spdk_put_io_channel(ctx.channel);
            spdk_bdev_close(ctx.desc);
            spdk_stop(0);
        }
        return;
    }

    unsafe {
        let rc = spdk_bdev_write(
            ctx.desc,
            ctx.channel,
            ctx.buf,
            ctx.count * 512,
            512,
            Some(io_complete),
            arg,
        );
        assert_eq!(rc, 0);
    };
    std::mem::forget(ctx);
}

#[derive(Debug)]
struct IoCtx {
    count: u64,
    desc: *mut spdk_bdev_desc,
    channel: *mut spdk_io_channel,
    buf: *mut c_void,
    start: std::time::Instant,
}

fn cb_read_some() {
    let d = Descriptor::open(BDEVNAME, true);
    let d = d.unwrap();
    let buf = d.dma_zmalloc(512).expect("failed to allocate buffer");
    let start = std::time::Instant::now();

    let ctx = Box::into_raw(Box::new(IoCtx {
        count: 0,
        desc: d.desc,
        channel: d.ch,
        buf: buf.buf,
        start: std::time::Instant::now(),
    }));

    let _ = unsafe {
        spdk_bdev_write(
            d.desc,
            d.ch,
            buf.buf,
            0,
            buf.len as u64,
            Some(io_complete),
            ctx as *const _ as *mut _,
        );
    };

    std::mem::forget(buf);
    std::mem::forget(d);
}
