use common::bdev_io;
use mayastor::bdev::nexus_create;
use mayastor::core::{
    mayastor_env_stop,
    MayastorCliArgs,
    MayastorEnvironment,
    Reactor,
};

pub mod common;

#[test]
fn nexus_nio() {
    common::mayastor_test_init();
    MayastorEnvironment::new(MayastorCliArgs::default()).init();
    Reactor::block_on(async move {
        nexus_create(
            "nexus0",
            48 * 1024 * 1024,
            None,
            &[
                "malloc:///malloc0?size_mb=64".into(),
                "malloc:///malloc1?size_mb=64".into(),
            ],
        )
            .await
            .unwrap();

    bdev_io::read_some("nexus0", 0, 0xff).await.expect_err("should fail");
    }).unwrap();

    mayastor_env_stop(0);
}
