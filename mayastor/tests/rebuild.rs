use mayastor::aio_dev::AioBdev;
use mayastor::descriptor::Descriptor;
use mayastor::mayastor_start;
use mayastor::nexus_uri::BdevType::Aio;
use mayastor::rebuild::CopyTask;
use std::process::Command;

static DISKNAME1: &str = "/tmp/disk1.img";
static BDEVNAME1: &str = "aio:///tmp/disk1.img?blk_size=512";

static DISKNAME2: &str = "/tmp/disk2.img";
static BDEVNAME2: &str = "aio:///tmp/disk2.img?blk_size=512";

#[test]
fn copy_task() {
    let log = mayastor::spdklog::SpdkLog::new();
    let _ = log.init();

    mayastor::CPS_INIT!();
    let args = vec!["-c", "../etc/test.conf"];

    // setup our test files

    let output = Command::new("truncate")
        .args(&["-s", "64m", DISKNAME1])
        .output()
        .expect("failed exec truncate");

    assert_eq!(output.status.success(), true);

    let output = Command::new("truncate")
        .args(&["-s", "64m", DISKNAME2])
        .output()
        .expect("failed exec truncate");

    assert_eq!(output.status.success(), true);

    let rc: i32 = mayastor_start("test", args, || {
        mayastor::executor::spawn(works());
    });

    //    assert_eq!(rc, 0);
    //
    //    let output = Command::new("rm")
    //        .args(&["-rf", DISKNAME1, DISKNAME2])
    //        .output()
    //        .expect("failed delete test file");
    //
    //    assert_eq!(output.status.success(), true);
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

    let mut copy_task = CopyTask::new(source, target).unwrap();

    CopyTask::start_rebuild(copy_task);
}
