use mayastor::aio_dev::AioBdev;
use mayastor::descriptor::Descriptor;
use mayastor::rebuild::RebuildTask;
use mayastor::scanner::{ScannerTask, ScannerTaskTrait};
use mayastor::{mayastor_start, spdk_stop};

static DISKNAME1: &str = "/tmp/disk1.img";
static BDEVNAME1: &str = "aio:///tmp/disk1.img?blk_size=512";

static DISKNAME2: &str = "/tmp/disk2.img";
static BDEVNAME2: &str = "aio:///tmp/disk2.img?blk_size=512";

mod common;
#[test]
fn copy_task() {
    common::mayastor_test_init();
    let args = vec!["rebuild_task"];

    // setup our test files
    common::create_disk(DISKNAME1, "64m");
    common::create_disk(DISKNAME2, "64m");

    let rc: i32 = mayastor_start("test", args, || {
        mayastor::executor::spawn(works());
    });

    assert_eq!(rc, 0);
    common::delete_disk(&[DISKNAME1.into(), DISKNAME2.into()])
}

async fn create_bdevs() {
    let source = AioBdev {
        name: BDEVNAME1.to_string(),
        file: DISKNAME1.to_string(),
        blk_size: 512,
    };

    let target = AioBdev {
        name: BDEVNAME2.to_string(),
        file: DISKNAME2.to_string(),
        blk_size: 512,
    };

    source.create().await.unwrap();
    target.create().await.unwrap();
}
async fn works() {
    create_bdevs().await;

    let source = Descriptor::open(BDEVNAME1, false).unwrap();
    let target = Descriptor::open(BDEVNAME2, true).unwrap();

    let copy_task = RebuildTask::new(source, target).unwrap();

    if let Some(r) = RebuildTask::start_rebuild(copy_task) {
        let _done = r.await;
    }

    scan().await;
}

async fn scan() {
    let source = Descriptor::open(BDEVNAME1, false).unwrap();
    let target = Descriptor::open(BDEVNAME2, true).unwrap();

    let scan = ScannerTask::new(source, target).unwrap();
    if let Some(r) = ScannerTask::start_task(scan, Some(0)) {
        let done = r.await;
        dbg!(done);
    }
}
