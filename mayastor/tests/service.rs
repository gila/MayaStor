use mayastor::{
    core::{CoreError, Cores, MayastorCliArgs, Mthread},
    subsys::Service,
};
pub mod common;
use common::MayastorTest;
use mayastor::{
    core::{Bdev, Share},
    nexus_uri::bdev_create,
    subsys::NVMF_TARGET,
};

use spdk_sys::spdk_env_get_core_count;
#[tokio::test]
async fn service_test() {
    let args = MayastorCliArgs {
        reactor_mask: "0x3".into(),
        log_components: vec!["all".into()],
        ..Default::default()
    };

    let _ms = MayastorTest::new(args);
    let service = Box::new(Service::new("test_service".into(), Cores::last()));
    let service = Box::leak(service);

    println!("current core: {}", Cores::current());
    println!("number of  cores: {}", unsafe { spdk_env_get_core_count() });

    let _last = Cores::last();

    service.with(move || {
        println!("running on {}", Cores::current());
        println!("Running on thread {:?}", Mthread::current());
    });

    let last = Cores::last();

    let rx = service.spawn_local::<_, _, CoreError>(async move {
        assert_eq!(Cores::current(), last.id());
        println!("running on {}", Cores::current());
        println!("Running on thread {:?}", Mthread::current());
        NVMF_TARGET.get().unwrap().start().await;
        Ok(())
    });

    assert_eq!(Mthread::current(), None);

    let _l = rx.unwrap().await;

    let rx = Mthread::get_init().spawn_local(async move {
        bdev_create("malloc:///malloc0?size_mb=100").await
    });

    let l = rx.unwrap().await;
    dbg!(l);

    let rx = service.spawn_local(async move {
        let bdev = Bdev::lookup_by_name("malloc0").unwrap();
        bdev.share_nvmf(None).await
    });

    let l = rx.unwrap().await;
    dbg!(l);

    let rx = Mthread::get_init().spawn_local(async move {
        bdev_create("nvmf://127.0.0.1:8420/nqn.2019-05.io.openebs:malloc0")
            .await
    });
    let l = rx.unwrap().await;
    dbg!(l);
}
