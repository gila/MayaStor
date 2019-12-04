use mayastor::{
    aio_dev::AioBdev,
    bdev::nexus::nexus_bdev::{nexus_create, nexus_lookup},
    descriptor::Descriptor,
    mayastor_start,
    mayastor_stop,
    rebuild::RebuildTask,
};
use std::{ffi::CString, rc::Rc, thread};

static DISKNAME1: &str = "/code/disk1.img";
static BDEVNAME1: &str = "aio:///code/disk1.img?blk_size=512";

static DISKNAME2: &str = "/code/disk2.img";
static BDEVNAME2: &str = "aio:///code/disk2.img?blk_size=512";
use mayastor::{
    event::{on_core, run_on_core},
    rebuild::RebuildState,
};
use spdk_sys::*;
use std::time::Duration;

mod common;
#[test]
fn copy_task() {
    common::mayastor_test_init();
    let args = vec!["rebuild_task", "-m", "0x3"];

    //  common::dd_random_file(DISKNAME1, 4096, 4096 * 1024);
    //  common::truncate_file(DISKNAME2, 4096 * 1024);

    let rc: i32 = mayastor_start("test", args, || {
        mayastor::executor::spawn(works());
    });

    assert_eq!(rc, 0);

    common::compare_files(DISKNAME1, DISKNAME2);
    //common::delete_file(&[DISKNAME1.into(), DISKNAME2.into()]);
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

    let ctx = run_on_core(0, copy_task, |task| {
        task.start_time = Some(std::time::SystemTime::now());
        match task.next() {
            Err(next) => {
                dbg!("{:?}", next);
                task.shutdown(false);
            }
            Ok(..) => {
                task.state = RebuildState::Running;
                task.start_progress_poller();
            }
        }
    });

    if let Ok(mut ctx) = ctx {
        let done = ctx.completed().await.unwrap();
        assert_eq!(done, true);
    }

    //    if let Ok(mut e) = RebuildTask::start_rebuild(copy_task) {
    //        let done = e.completed().await.unwrap();
    //        assert_eq!(done, true);
    //    }

    let _ = source.destroy().await;
    let _ = target.destroy().await;
}

async fn create_nexus() {
    let ch = vec![BDEVNAME1.to_string(), BDEVNAME2.to_string()];
    nexus_create("rebuild_nexus", 64 * 1024 * 1024, None, &ch)
        .await
        .unwrap();
}

async fn rebuild_nexus_online() {
    let nexus = nexus_lookup("rebuild_nexus").unwrap();

    // fault a child which will allow us to rebuild it

    nexus.fault_child(BDEVNAME1).await.unwrap();
    nexus.init_rebuild().unwrap();
    nexus.start_rebuild().unwrap();

    nexus.rebuild_completion().await.unwrap();

    //  mayastor_stop(0);
}

async fn works() {
    //create_nexus().await;
    rebuild_direct().await;
    //    on_core(1, || {
    //        let nexus = nexus_lookup("rebuild_nexus").unwrap();
    //        loop {
    //            std::thread::sleep(Duration::from_millis(1000));
    //            nexus.log_progress();
    //        }
    //    });
    //
    //rebuild_nexus_online().await;
}
