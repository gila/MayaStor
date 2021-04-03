pub mod common;
use common::MayastorTest;
use mayastor::{
    self,
    bdev::nexus::child::State,
    core::MayastorCliArgs,
    subsys::child::Inventory,
};

#[tokio::test]
async fn new_child() {
    let ms = MayastorTest::new(MayastorCliArgs::default());
    // Create a nexus with a single child
    ms.spawn(async {
        let child = Inventory::get()
            .create("malloc:///malloc0?size_mb=64")
            .await
            .unwrap();

        let state = child.lock().unwrap().open();
        dbg!(&child);
    })
    .await;

    // ms.spawn(async {
    //     let c = CHILD_INVENTORY.lookup("malloc0").unwrap();
    //     let mut c = c.lock().unwrap();
    //     assert_eq!(c.state(), State::Open);
    //
    //     c.close().unwrap();
    //     assert_eq!(c.state(), State::Init);
    //     dbg!(&c);
    //     c.open().unwrap();
    //     dbg!(&c);
    //     assert_eq!(c.state(), State::Open);
    // })
    // .await;
    //
    // ms.spawn(async {
    //     let c = CHILD_INVENTORY.lookup("malloc0").unwrap();
    //     let mut c = c.lock().unwrap();
    //     c.fault(Reason::Missing).unwrap();
    //     assert_eq!(c.state(), State::Faulted(Reason::Missing));
    //     dbg!(c.destroy().await.unwrap());
    // })
    // .await;
    //
    // ms.spawn(async {
    //     let c = CHILD_INVENTORY.lookup("malloc0").unwrap();
    //     CHILD_INVENTORY.drop_all();
    //     drop(c);
    // })
    // .await;
}
