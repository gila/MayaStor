use mayastor::{
    aio_dev::AioBdev,
    bdev::nexus::nexus_bdev::{nexus_create, nexus_lookup},
    descriptor::Descriptor,
    mayastor_start,
    mayastor_stop,
    rebuild::RebuildTask,
};
use std::rc::Rc;

static DISKNAME1: &str = "/code/disk1.img";
static BDEVNAME1: &str = "aio:///code/disk1.img?blk_size=512";

static DISKNAME2: &str = "/code/disk2.img";
static BDEVNAME2: &str = "aio:///code/disk2.img?blk_size=512";

mod common;
#[test]
fn copy_task() {
    common::mayastor_test_init();
    let args = vec!["rebuild_task", "-L", "bdev", "-m", "0x2"];

    common::dd_random_file(DISKNAME1, 4096, 4096 * 1024);
    common::truncate_file(DISKNAME2, 4096 * 1024);

    let rc: i32 = mayastor_start("test", args, || {
        mayastor::executor::spawn(works());
    });

    assert_eq!(rc, 0);

    common::compare_files(DISKNAME1, DISKNAME2);
    common::delete_file(&[DISKNAME1.into(), DISKNAME2.into()]);
}

async fn rebuild_direct() {
    let source = AioBdev {
        name: BDEVNAME1.to_string(),
        file: DISKNAME1.to_string(),
        blk_size: 4096,
    };

    let target = AioBdev {
        name: BDEVNAME2.to_string(),
        file: DISKNAME2.to_string(),
        blk_size: 4096,
    };

    if source.clone().create().await.is_err() {
        panic!("failed to create source device for rebuild test");
    }

    if target.clone().create().await.is_err() {
        panic!("failed to create target device for rebuild test");
    }

    let sourcebd = Descriptor::open(BDEVNAME1, false).unwrap();
    let targetbd = Descriptor::open(BDEVNAME2, true).unwrap();

    let copy_task =
        RebuildTask::new(Rc::new(sourcebd), Rc::new(targetbd)).unwrap();

    if let Ok(mut e) = RebuildTask::start_rebuild(copy_task) {
        let done = e.completed().await.unwrap();
        assert_eq!(done, true);
    }

    let _ = source.destroy().await;
    let _ = target.destroy().await;
}

async fn create_nexus() {
    let ch = vec![BDEVNAME1.to_string(), BDEVNAME2.to_string()];
    nexus_create("rebuild_nexus", 64 * 1024 * 1024, None, &ch)
        .await
        .unwrap();
}

async fn rebuild_nexus_offline() {
    let nexus = nexus_lookup("rebuild_nexus").unwrap();
    let mut v = nexus.get_descriptors();

    let copy_task =
        RebuildTask::new(v.pop().unwrap(), v.pop().unwrap()).unwrap();

    if let Ok(mut r) = RebuildTask::start_rebuild(copy_task) {
        let done = r.completed().await;
        assert_eq!(done.unwrap(), true);
    }
}

async fn rebuild_nexus_online() {
    let nexus = nexus_lookup("rebuild_nexus").unwrap();
    nexus.fault_child(BDEVNAME1).await.unwrap();
    nexus.init_rebuild().unwrap();
    nexus.start_rebuild().unwrap();
    nexus.rebuild_completion().await.unwrap();

    mayastor_stop(0);
}

async fn works() {
    rebuild_direct().await;
    create_nexus().await;
    rebuild_nexus_offline().await;
    rebuild_nexus_online().await;
    //nexus.offline_child(BDEVNAME1).await;
}
