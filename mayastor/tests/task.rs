use futures::{
    future::err,
    task::{Context, Waker},
    Future,
    Poll,
};
use mayastor::{
    bdev::{
        bdev_lookup_by_name,
        nexus::nexus_bdev::{nexus_create, nexus_lookup},
        Bdev,
    },
    environment::{
        args::MayastorCliArgs,
        env::{mayastor_env_stop, MayastorEnvironment},
    },
    executor::spawn,
    poller::{PollFunction, PollTask, SetTimeout},
};
use spdk_sys::{
    spdk_bdev_desc,
    spdk_bdev_desc_get_bdev,
    spdk_bdev_free_io,
    spdk_bdev_get_io_channel,
    spdk_bdev_io,
    spdk_bdev_open,
    spdk_bdev_read_blocks,
    spdk_io_channel,
};
use std::{
    cell::{Cell, RefCell},
    os::raw::c_void,
    pin::Pin,
};

pub mod common;

static DISKNAME1: &str = "/tmp/disk1.img";
static BDEVNAME1: &str = "aio:///tmp/disk1.img?blk_size=512";

static DISKNAME2: &str = "/tmp/disk2.img";
static BDEVNAME2: &str = "aio:///tmp/disk2.img?blk_size=512";
pub struct Task {
    state: RefCell<TaskState>,
    counter: Cell<u32>,
    poller: Option<PollTask>,
}

impl Task {
    pub fn run(&self) {
        let mut state = self.state.borrow_mut();
        let mut cnt = self.counter.get();
        if self.counter.get() == 1000 {
            state.completed = true;
        }
        cnt += 1;
        self.counter.set(cnt);

        if let Some(waker) = state.waker.take() {
            waker.wake();
        }

        println!("running..")
    }

    pub fn new() -> Self {
        let fut = Self {
            counter: Cell::new(0),
            state: RefCell::new(TaskState {
                completed: false,
                waker: None,
            }),
            poller: None,
        };

        fut
    }
}

#[derive(Debug)]
pub struct TaskState {
    completed: bool,
    waker: Option<Waker>,
}

impl Future for Task {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        dbg!(&self.state);

        if self.state.borrow().completed == true {
            Poll::Ready(())
        } else {
            let mut state = self.state.borrow_mut();
            state.waker = Some(cx.waker().clone());

            drop(state);
            self.run();
            Poll::Pending
        }
    }
}

#[derive(Debug)]
pub struct MayaChannel(*mut spdk_io_channel);

/// new type around desc
/// The descriptor implements copy and clone
#[derive(Debug, Clone, Copy)]
pub struct MayaDesc(*mut spdk_bdev_desc);

impl MayaDesc {
    pub fn as_ptr(&self) -> *mut spdk_bdev_desc {
        self.0
    }

    pub fn get_channel(&self) -> Option<MayaChannel> {
        return if self.0.is_null() {
            None
        } else {
            Some(MayaChannel {
                0: unsafe { spdk_bdev_get_io_channel(self.0) },
            })
        };
    }

    pub fn get_bdev(&self) -> Option<Bdev> {
        return if self.0.is_null() {
            None
        } else {
            Some(Bdev {
                inner: unsafe { spdk_bdev_desc_get_bdev(self.0) },
            })
        };
    }

    pub fn into_handle(self) -> Option<BdevHandle> {
        Some(BdevHandle {
            desc: self,
            ch: self.get_channel().unwrap(),
        })
    }
}

pub struct BdevHandle {
    desc: MayaDesc,
    ch: MayaChannel,
}

impl BdevHandle {
    pub fn read(&self, offset: u64, num_blocks: u64) -> TaskState {
        extern "C" fn read_complete(
            io: *mut spdk_bdev_io,
            success: bool,
            _ctx: *mut c_void,
        ) {
            if success {
                println!("IO OK");
            } else {
                println!("IO FAIL");
            }

            unsafe {
                spdk_bdev_free_io(io);
            }
        }

        let error = unsafe {
            spdk_bdev_read_blocks(
                self.desc.as_ptr(),
                self.ch.0,
                std::ptr::null_mut(),
                offset,
                num_blocks,
                Some(read_complete),
                std::ptr::null_mut(),
            )
        };

        TaskState {
            completed: false,
            waker: None,
        }
    }
}

#[derive(Debug)]
pub enum SegmentState {
    CLEAN,
    DIRTY,
    SYNCED,
}

#[derive(Debug)]
pub struct Segment {
    state: SegmentState,
    from: u64,
    to: u64,
}

#[derive(Debug)]
pub struct CopyTask {
    source: MayaDesc,
    target: MayaDesc,
    segment: Segment,
}

impl CopyTask {
    pub fn new(source: MayaDesc, target: MayaDesc, segment: Segment) -> Self {
        Self {
            source,
            target,
            segment,
        }
    }
}

#[test]
fn task_test() {
    common::mayastor_test_init();
    let _args = vec!["rebuild_task", "-m", "0x3", "-L", "bdev", "-L", "aio"];

    common::delete_file(&[DISKNAME1.into(), DISKNAME2.into()]);
    common::truncate_file(DISKNAME1, 64 * 1024);
    common::truncate_file(DISKNAME2, 64 * 1024);

    let rc = MayastorEnvironment::new(MayastorCliArgs::default())
        .start(|| mayastor::executor::spawn(works()))
        .unwrap();

    common::delete_file(&[DISKNAME1.into(), DISKNAME2.into()]);
    common::clean_up_temp();
    //  assert_eq!(rc, 0);
}

async fn create_nexus() {
    let ch = vec![BDEVNAME1.to_string(), BDEVNAME2.to_string()];
    nexus_create("nexus", 64 * 1024 * 1024, None, &ch)
        .await
        .unwrap();
}

async fn works() {
    create_nexus().await;

    let b = bdev_lookup_by_name("nexus").unwrap();

    let mut desc = std::ptr::null_mut();

    let rc = unsafe {
        spdk_bdev_open(b.as_ptr(), true, None, std::ptr::null_mut(), &mut desc)
    };

    let md = MayaDesc(desc).into_handle().unwrap();

    md.read(0, 4, Task::new());
}
