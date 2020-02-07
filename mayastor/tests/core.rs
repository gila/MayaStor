use std::{collections::HashMap, pin::Pin};

use futures::Future;

use mayastor::{
    bdev::{nexus_create, nexus_lookup},
    core::{
        mayastor_env_stop,
        Bdev,
        BdevHandle,
        Cores,
        MayastorCliArgs,
        MayastorEnvironment,
        Reactor,
        Reactors,
    },
    nexus_uri::{bdev_create, bdev_destroy},
};
use spdk_sys::spdk_get_thread;

static DISKNAME1: &str = "/tmp/disk1.img";
static BDEVNAME1: &str = "aio:///tmp/disk1.img?blk_size=512";

static DISKNAME2: &str = "/tmp/disk2.img";
static BDEVNAME2: &str = "aio:///tmp/disk2.img?blk_size=512";

pub mod common;

#[derive(Debug, PartialEq)]
pub enum Test {
    Pass,
    Fail,
}

#[test]
fn core() {
    common::mayastor_test_init();
    common::truncate_file(DISKNAME1, 64 * 1024);
    common::truncate_file(DISKNAME2, 64 * 1024);

    let ms = MayastorEnvironment::new(MayastorCliArgs {
        log_components: vec!["nvmf".into()],
        reactor_mask: "0x1".to_string(),
        ..Default::default()
    }).start(|| {
        let r = Reactors::get_by_core(Cores::first()).unwrap();

        r.with(|| {
            let h = Reactor::block_on(works());
        });

        mayastor_env_stop(0);
    });

    common::delete_file(&[DISKNAME1.into(), DISKNAME2.into()]);
}

async fn create_nexus() {
    let ch = vec![BDEVNAME1.to_string(), BDEVNAME2.to_string()];
    let _ = nexus_create("core_nexus", 64 * 1024 * 1024, None, &ch)
        .await
        .unwrap();
    //.expect("failed to crate nexus");
}

async fn works() {
    assert_eq!(Bdev::lookup_by_name("core_nexus").is_none(), true);
    create_nexus().await;
    let b = Bdev::lookup_by_name("core_nexus").unwrap();
    assert_eq!(b.name(), "core_nexus");

    let desc = Bdev::open_by_name("core_nexus", false).unwrap();
    let channel = desc.get_channel().expect("failed to get IO channel");
    drop(channel);
    drop(desc);

    let n = nexus_lookup("core_nexus").expect("nexus not found");
    n.destroy().await;
}

async fn multiple_open() {
    create_nexus().await;

    let n = nexus_lookup("core_nexus").expect("failed to lookup nexus");

    let d1 = Bdev::open_by_name("core_nexus", true)
        .expect("failed to open first desc to nexus");
    let d2 = Bdev::open_by_name("core_nexus", true)
        .expect("failed to open second desc to nexus");

    let ch1 = d1.get_channel().expect("failed to get channel!");
    let ch2 = d2.get_channel().expect("failed to get channel!");
    drop(ch1);
    drop(ch2);

    // we must drop the descriptors before we destroy the nexus
    drop(dbg!(d1));
    drop(dbg!(d2));
    n.destroy().await;
}

async fn handle_test() {
    bdev_create(BDEVNAME1).await.expect("failed to create bdev");
    let hdl2 = BdevHandle::open(BDEVNAME1, true, true)
        .expect("failed to create the handle!");
    let hdl3 = BdevHandle::open(BDEVNAME1, true, true);
    assert_eq!(hdl3.is_err(), true);

    // we must drop the descriptors before we destroy the nexus
    drop(hdl2);
    drop(hdl3);

    bdev_destroy(BDEVNAME1)
        .await
        .expect("failed to destroy bdev");
}
