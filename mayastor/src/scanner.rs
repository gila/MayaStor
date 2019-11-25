use std::os::raw::c_void;

use spdk_sys::{
    spdk_bdev_free_io, spdk_bdev_io, spdk_bdev_read_blocks,
    spdk_bdev_writev_blocks, spdk_env_get_current_core, spdk_event_allocate,
    spdk_event_call, spdk_poller, spdk_poller_register, spdk_poller_unregister,
};

use crate::bdev::nexus::nexus_io::Bio;
use crate::bdev::nexus::Error;
use crate::descriptor::Descriptor;
use futures::channel::oneshot;

#[derive(Debug)]
pub enum BlockTraverser {
    Init,
    Reading,
    Writing,
    Error,
    Completed,
}

pub trait ScannerTaskTrait {
    extern "C" fn source_complete(
        io: *mut spdk_bdev_io,
        success: bool,
        ctx: *mut c_void,
    );
    extern "C" fn target_complete(
        io: *mut spdk_bdev_io,
        success: bool,
        ctx: *mut c_void,
    );

    fn task_from_ctx(ctx: *mut c_void) -> Box<ScannerTask> {
        unsafe { Box::from_raw(ctx as *const _ as *mut ScannerTask) }
    }
}

/// struct that holds the state of a copy task. This struct
/// is used during rebuild.
#[derive(Debug)]
pub struct ScannerTask {
    /// the source where to copy from
    source: Descriptor,
    /// the target where to copy to
    target: Descriptor,
    /// the last LBA for which an io copy has been submitted
    current_lba: u64,
    /// the IO we are rebuilding
    source_io: Option<*mut spdk_bdev_io>,
    /// an optional sender for us to call send our completion too
    sender: Option<oneshot::Sender<bool>>,
    /// progress reported to logs
    progress: Option<*mut spdk_poller>,
    // queue IO to dispatch during next poller run
    // queue: VecDeque<DmaBuf>,
    traverser: BlockTraverser,
    num_segments: u64,
    remainder: u32,
}

impl ScannerTask {
    /// return a new rebuild task
    pub fn new(
        source: Descriptor,
        target: Descriptor,
    ) -> Result<Box<Self>, Error> {
        let current_lba = 0;

        // if the target is to small, we bail out. A future extension is to see if we can grow;
        // the target to match the size of the source.

        if target.get_bdev().num_blocks() < source.get_bdev().num_blocks() {
            let error = format!(
                "source {} is larger than the target {}",
                source.get_bdev().name(),
                target.get_bdev().name()
            );
            error!("{}", &error);
            return Err(Error::Invalid(error));
        }

        let num_blocks = target.get_bdev().num_blocks();

        Ok(Box::new(Self {
            source,
            target,
            current_lba,
            source_io: None,
            sender: None,
            remainder: (num_blocks % 128) as u32,
            num_segments: num_blocks / 128,
            progress: None,
            traverser: BlockTraverser::Init,
        }))
    }

    fn stop_progress(&mut self) {
        if let Some(mut poller) = self.progress.take() {
            unsafe {
                spdk_poller_unregister(&mut poller);
            }
        }
    }

    /// send the waiter that we completed successfully
    fn send_completion(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(true);
        }
    }

    extern "C" fn scan_progress(task: *mut c_void) -> i32 {
        let task = Self::task_from_ctx(task);
        info!("scan: current lba {}", task.current_lba);
        std::mem::forget(task);
        0
    }

    extern "C" fn _start_scanning(copy_task: *mut c_void, _arg2: *mut c_void) {
        let mut task = Self::task_from_ctx(copy_task);

        match task.dispatch_next_segment() {
            Err(next) => {
                error!("{:?}", next);
                let _ = task.sender.unwrap().send(false);
            }
            Ok(..) => {
                std::mem::forget(task);
            }
        }
    }

    pub fn start_task(
        mut task: Box<ScannerTask>,
        core: Option<u32>,
    ) -> Option<oneshot::Receiver<bool>> {
        let core = if let Some(core) = core { core } else { 0 };

        let (s, r) = oneshot::channel::<bool>();
        task.sender = Some(s);

        let poller = unsafe {
            spdk_poller_register(
                Some(Self::scan_progress),
                &*task as *const _ as *mut _,
                1_000_000,
            )
        };

        if poller.is_null() {
            error!("failed to register poller for rebuild task");
            return None;
        }

        task.progress = Some(poller);

        let event = unsafe {
            spdk_event_allocate(
                core,
                Some(Self::_start_scanning),
                Box::into_raw(task) as *mut c_void,
                std::ptr::null_mut(),
            )
        };

        if event.is_null() {
            return None;
        }

        // cant fail?
        unsafe {
            // the event will be put back into the mem pool when the reactor de-queues it
            spdk_event_call(event);
        }

        Some(r)
    }
    fn dispatch_next_segment(&mut self) -> Result<bool, Error> {
        let num_blocks = if self.num_segments > 0 {
            128
        } else {
            self.remainder
        };
        if self.current_lba < self.source.get_bdev().num_blocks() {
            let ret = unsafe {
                spdk_bdev_read_blocks(
                    self.source.desc,
                    self.source.ch,
                    std::ptr::null_mut(),
                    self.current_lba,
                    num_blocks as u64,
                    Some(Self::source_complete),
                    &*self as *const _ as *mut _,
                )
            };

            // if we failed to dispatch the IO, we will redo it later return Ok(false)
            if ret == 0 {
                self.traverser = BlockTraverser::Reading;
                self.current_lba += num_blocks as u64;
                Ok(false)
            } else {
                // for now fail on all errors; typically with ENOMEM we should retry
                // however, we want to delay this so likely use a (one time) poller?
                Err(Error::Internal("failed to dispatch IO".into()))
            }
        } else {
            self.traverser = BlockTraverser::Completed;
            assert_eq!(self.current_lba, self.source.get_bdev().num_blocks());
            trace!("scan task completed! \\o/");
            Ok(true)
        }
    }

    fn source_io_hold(
        &mut self,
        io: *mut spdk_bdev_io,
    ) -> Option<*mut spdk_bdev_io> {
        self.source_io.replace(io)
    }
}

impl ScannerTaskTrait for ScannerTask {
    extern "C" fn source_complete(
        io: *mut spdk_bdev_io,
        success: bool,
        ctx: *mut c_void,
    ) {
        let mut task = Self::task_from_ctx(ctx);
        if success {
            task.source_io_hold(io);

            let num_blocks = if task.num_segments > 0 {
                128
            } else {
                task.remainder
            };
            unsafe {
                spdk_bdev_read_blocks(
                    task.target.desc,
                    task.target.ch,
                    std::ptr::null_mut(),
                    task.current_lba - num_blocks as u64,
                    num_blocks as u64,
                    Some(Self::target_complete),
                    Box::into_raw(task) as *const _ as *mut _,
                );
            }
        } else {
            unsafe {
                spdk_bdev_free_io(io);
            }
        }
    }

    extern "C" fn target_complete(
        io: *mut spdk_bdev_io,
        success: bool,
        ctx: *mut c_void,
    ) {
        let mut task = Self::task_from_ctx(ctx);

        let sio = Bio::from(task.source_io.take().unwrap());
        let tio = Bio::from(io);

        let len = unsafe { (*tio.iovs()).iov_len as isize };

        let siov = sio.iovs() as *const usize;
        let tiov = tio.iovs() as *const usize;
        unsafe {
            for i in 0..len {
                assert_eq!(*siov.offset(i), *siov.offset(i));
            }
        }

        Bio::io_free(sio.io);
        Bio::io_free(tio.io);

        match task.dispatch_next_segment() {
            Ok(next) => {
                if next {
                    info!("compare completed");
                    task.stop_progress();
                    task.send_completion();
                } else {
                    std::mem::forget(task)
                }
            }

            Err(..) => {
                panic!("meh");
            }
        }
    }
}
