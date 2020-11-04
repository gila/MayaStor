pub mod common;
use common::{get_mayastor_test, Builder as ComposeBuilder, RpcHandle};
use mayastor::{
    bdev::{nexus_create, nexus_lookup},
    core::{Bdev, Builder, JobQueue, MayastorCliArgs},
    nexus_uri::bdev_destroy,
};
use pin_utils::core_reexport::time::Duration;
use rpc::mayastor::{BdevShareRequest, BdevUri};
use std::sync::Arc;

async fn create_nexus(mut hdls: Vec<RpcHandle>, queue: Arc<JobQueue>) {
    for h in &mut hdls {
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

    get_mayastor_test().send(async move {
        nexus_create(
            "nexus0",
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

        let bdev = Bdev::lookup_by_name("nexus0").unwrap();
        let job = Builder::new().bdev(bdev).qd(64).io_size(512).build().await;

        queue.start(job);
    });
}

#[tokio::test]
async fn io_driver() {
    let queue = Arc::new(JobQueue::new());
    let ms = get_mayastor_test();

    let test = ComposeBuilder::new()
        .name("cargo-test")
        .network("10.1.0.0/16")
        .add_container("ms2")
        .add_container("ms1")
        .with_clean(true)
        .build()
        .await
        .unwrap();

    // get the handles if needed, to invoke methods to the containers
    let hdls = test.grpc_handles().await.unwrap();
    create_nexus(hdls.clone(), queue.clone()).await;

    // create and share a bdev on each container
    tokio::time::delay_for(Duration::from_secs(5)).await;

    ms.spawn(async move {
        let bdev = Bdev::lookup_by_name("nexus0").unwrap();
        let result = bdev.stats().await.unwrap();
        println!("{:?}", result);
    })
    .await;

    ms.spawn(async move {
        let nexus = nexus_lookup("nexus0").unwrap();
        nexus
            .remove_child(&format!(
                "nvmf://{}:8420/nqn.2019-05.io.openebs:disk0",
                hdls[0].endpoint.ip()
            ))
            .await
            .unwrap();
    })
    .await;

    ms.spawn(async move {
        let bdev = Bdev::lookup_by_name("nexus0").unwrap();
        let result = bdev.stats().await.unwrap();
        println!("{:?}", result);
    })
    .await;

    tokio::time::delay_for(Duration::from_secs(5)).await;
    queue.stop("nexus0");

    ms.spawn(async move {
        let bdev = Bdev::lookup_by_name("nexus0").unwrap();
        let result = bdev.stats().await.unwrap();
        println!("{:?}", result);
    })
    .await;

    ms.spawn(async {
        nexus_lookup("nexus0").unwrap().destroy().await.unwrap();
    })
    .await;
}
