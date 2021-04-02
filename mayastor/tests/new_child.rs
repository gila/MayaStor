pub mod common;
use common::MayastorTest;
use mayastor::{
    bdev::child::{Child, CHILD_LIST},
    core::MayastorCliArgs,
    nexus_uri::bdev_create,
};

#[tokio::test]
async fn new_child() {
    let ms = MayastorTest::new(MayastorCliArgs::default());
    // Create a nexus with a single child
    ms.spawn(async {
        let bdev = bdev_create("malloc:///malloc0?size_mb=64").await.unwrap();
        dbg!(&bdev);
        let child = Child::new(bdev).unwrap();
        dbg!(child);
    })
    .await;

    ms.spawn(async {
        let c = CHILD_LIST.lookup("malloc0").unwrap();
        dbg!(c);
    })
    .await;
}
