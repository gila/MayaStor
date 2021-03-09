use common::bdev_io;
use mayastor::bdev::{nexus_create, nexus_lookup};
use mayastor::core::{mayastor_env_stop, MayastorCliArgs, MayastorEnvironment, Reactor, Reactors, BdevHandle};

pub mod common;

#[test]
fn nexus_nio() {
    common::mayastor_test_init();
    let mut args = MayastorCliArgs::default();
    args.log_components = vec!["bdev".into(), "bdev_malloc".into()];
    MayastorEnvironment::new(args).init();
    Reactor::block_on(async move {
        nexus_create(
            "nexus0",
            48 * 1024 * 1024,
            None,
            &[
                "malloc:///malloc0?size_mb=64".into(),
                //"malloc:///malloc1?size_mb=64".into(),
            ],
        )
            .await
            .unwrap();

    }).unwrap();

    Reactor::block_on( async {

        bdev_io::write_some("nexus0", 0, 0xff).await.unwrap();
        bdev_io::read_some("nexus0", 0, 0xff).await.unwrap();

        mayastor_env_stop(0);
    }).unwrap();

}
