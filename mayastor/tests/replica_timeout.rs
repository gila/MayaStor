#![allow(unused_assignments)]

use common::{bdev_io, Builder, MayastorTest};
use mayastor::{
    bdev::{nexus_create, nexus_lookup},
    core::MayastorCliArgs,
    subsys::Config,
    core::Reactor,
    core::Mthread,
};

use rpc::mayastor::{BdevShareRequest, BdevUri, Null};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::time::{delay_for, Duration};

pub mod common;
static NXNAME: &str = "nexus";

#[tokio::test]
async fn replica_stop_cont() {
    let test = Builder::new()
        .name("cargo-test")
        .network("10.1.0.0/16")
        .add_container("ms2")
        .add_container("ms1")
        .with_clean(true)
        .build()
        .await
        .unwrap();

    // get the handles if needed, to invoke methods to the containers
    let mut hdls = test.grpc_handles().await.unwrap();

    // create and share a bdev on each container
    for h in &mut hdls {
        h.bdev.list(Null {}).await.unwrap();
        h.bdev
            .create(BdevUri {
                uri: "malloc:///disk0?size_mb=100".into(),
            })
            .await
            .unwrap();
        h.bdev
            .share(BdevShareRequest {
                name: "disk0".into(),
                proto: "nvmf".into(),
            })
            .await
            .unwrap();
    }

    common::mayastor_test_init();
    let mayastor = MayastorTest::old_new(MayastorCliArgs::default());
    Mthread::unaffinitize();

    mayastor
        .spawn(async move {
            nexus_create(
                NXNAME,
                1024 * 1024 * 50,
                None,
                &[
                    format!(
                        "nvmf://{}:8420/nqn.2019-05.io.openebs:disk0",
                        hdls[0].endpoint.ip()
                    ),
                    format!(
                        "nvmf://{}:8420/nqn.2019-05.io.openebs:disk0",
                        hdls[1].endpoint.ip()
                    ),
                ],
            )
            .await
            .unwrap();
            bdev_io::write_some(NXNAME, 0, 0xff).await.unwrap();
            bdev_io::read_some(NXNAME, 0, 0xff).await.unwrap();
        })
        .await;

    test.pause("ms1").await.unwrap();
    delay_for(Duration::from_secs(5)).await;

    mayastor.send(async {
            dbg!(bdev_io::read_some(NXNAME, 0, 0xff).await);
            dbg!(bdev_io::read_some(NXNAME, 0, 0xff).await);
        });

    delay_for(Duration::from_secs(5)).await;
    println!("io submitted unfreeze container...");
    test.un_pause("ms1").await.unwrap();
    delay_for(Duration::from_secs(5)).await;
}
