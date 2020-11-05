use std::sync::Arc;

use once_cell::sync::OnceCell;
use tokio::time::Duration;

use common::{Builder as ComposeBuilder, ComposeTest, MayastorTest};
use mayastor::{
    bdev::{nexus_create, nexus_lookup},
    core::{Bdev, Builder, JobQueue, MayastorCliArgs},
};
use rpc::mayastor::{BdevShareRequest, BdevUri};

pub mod common;

static DOCKER_COMPOSE: OnceCell<ComposeTest> = OnceCell::new();
static MAYASTOR: OnceCell<MayastorTest> = OnceCell::new();

async fn create_work(queue: Arc<JobQueue>) {
    let mut hdls = DOCKER_COMPOSE.get().unwrap().grpc_handles().await.unwrap();
    let ms = MAYASTOR.get().unwrap();

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

    ms.spawn(async move {
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
        let job = Builder::new()
            .core(1)
            .bdev(bdev)
            .qd(64)
            .io_size(512)
            .build()
            .await;

        queue.start(job);

        let job = Builder::new()
            .core(0)
            .uri("malloc:///disk0?size_mb=100")
            .qd(64)
            .io_size(512)
            .build()
            .await;

        queue.start(job);
    })
    .await
}

async fn stats() {
    let ms = MAYASTOR.get().unwrap();
    ms.spawn(async move {
        let bdev = Bdev::bdev_first().unwrap().into_iter();
        for b in bdev {
            let result = b.stats().await.unwrap();
            println!("{}: {:?}", b.name(), result);
        }
    })
    .await;
}

#[tokio::test]
async fn io_driver() {
    let queue = Arc::new(JobQueue::new());

    let compose = ComposeBuilder::new()
        .name("cargo-test")
        .network("10.1.0.0/16")
        .add_container("ms2")
        .add_container("ms1")
        .with_clean(true)
        .build()
        .await
        .unwrap();

    let mayastor_test = MayastorTest::new(MayastorCliArgs {
        log_components: vec!["all".into()],
        reactor_mask: "0x3".to_string(),
        no_pci: true,
        grpc_endpoint: "0.0.0.0".to_string(),
        ..Default::default()
    });

    DOCKER_COMPOSE.set(compose).unwrap();
    let ms = MAYASTOR.get_or_init(|| mayastor_test);

    create_work(queue.clone()).await;
    tokio::time::delay_for(Duration::from_secs(1)).await;
    tokio::time::delay_for(Duration::from_secs(2)).await;
    // create and share a bdev on each container

    queue.stop_all().await;
    stats().await;
    ms.spawn(nexus_lookup("nexus0").unwrap().destroy())
        .await
        .unwrap();

    DOCKER_COMPOSE.get().unwrap().down();
}
