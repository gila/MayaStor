use std::os::raw::c_void;

use spdk_sys::{
    spdk_bdev_free_io, spdk_bdev_io, spdk_bdev_read_blocks,
    spdk_bdev_writev_blocks, spdk_env_get_current_core, spdk_event_allocate,
    spdk_event_call, spdk_poller, spdk_poller_register, spdk_poller_unregister,
};

use spdk_sys::*;

use crate::bdev::nexus::nexus_io::Bio;
use crate::bdev::nexus::Error;
use crate::descriptor::{Descriptor, DmaBuf};
use futures::channel::oneshot;
use std::convert::TryInto;
use crate::scanner::BlockTraverser;
use std::time::SystemTime;

/// struct that holds the state of a copy task. This struct
/// is used during rebuild.
#[derive(Debug)]
pub struct RebuildTask {
    /// the source where to copy from
    pub source: Descriptor,
    /// the target where to copy to
    pub target: Descriptor,
    /// the last LBA for which an io copy has been submitted
    current_lba: u64,
    previous_lba: u64,
    /// the IO we are rebuilding
    source_io: Option<*mut spdk_bdev_io>,
    /// an optional sender for us to call send our completion too
    sender: Option<oneshot::Sender<bool>>,
    /// progress reported to logs
    progress: Option<*mut spdk_poller>,
    num_segments: u64,
    remainder: u32,
    buf: DmaBuf,
    blocks_per_segment: u32,
    start_time: Option<SystemTime>,
    // queue IO to dispatch during next poller run
    // queue: VecDeque<DmaBuf>,
}


impl RebuildTask {
    fn shutdown(&mut self, success: bool) {
        self.stop_progress();
        self.send_completion(success);
    }

    extern "C" fn write_complete(
        io: *mut spdk_bdev_io,
        success: bool,
        arg: *mut c_void,
    ) {
        let mut task = unsafe { Box::from_raw(arg as *mut RebuildTask) };
        Bio::io_free(io);

        match task.dispatch_next_segment() {
            Ok(next) => {
                if next {
                    let elapsed = task.start_time.unwrap().elapsed().unwrap();
                    let mbs = (task.source.get_bdev().block_len() as u64 * task.source.get_bdev().num_blocks()) >> 20;
                    info!(
                        "Rebuild completed after {:.} seconds total of {} ({}MBs) from {} to {}",
                        elapsed.as_secs(),
                        mbs,
                        mbs / elapsed.as_secs(),
                        task.source.get_bdev().name(),
                        task.target.get_bdev().name());

                    task.shutdown(success)
                } else {
                    // we are not done yet, forget the task to avoid dropping
                    std::mem::forget(task);
                }
            }
            Err(e) => {
                dbg!(e);
                // task will be dropped
                panic!("error in rebuild");
            }
        }
    }

    /// callback when the read of the rebuild progress has completed
    extern "C" fn read_complete(
        io: *mut spdk_bdev_io,
        success: bool,
        ctx: *mut c_void,
    ) {
        let mut task = unsafe { Box::from_raw(ctx as *mut RebuildTask) };
        if success {
            let bio = Bio::from(io);

            let rc = unsafe {
                spdk_bdev_write(
                    task.target.desc,
                    task.target.ch,
                    task.buf.buf,
                    bio.offset() * task.source.get_bdev().block_len() as u64,
                    bio.num_blocks() * task.source.get_bdev().block_len() as u64,
                    Some(Self::write_complete),
                    Box::into_raw(task) as *const _ as *mut _,
                )
            };

            // queue the IO

            if rc != 0 {
                panic!("ret {}", rc);
            }
        } else {
            task.shutdown(false);
        }

        Bio::io_free(io);
    }

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
        let block_len = target.get_bdev().block_len();
        let blocks_per_segment = u64::from(SPDK_BDEV_LARGE_BUF_MAX_SIZE / block_len);

        let num_segments = num_blocks / blocks_per_segment as u64;
        let remainder = num_blocks % blocks_per_segment;

        let buf = source
            .dma_malloc((blocks_per_segment * source.get_bdev().block_len() as u64) as usize)
            .unwrap();

        Ok(Box::new(Self {
            blocks_per_segment: blocks_per_segment as u32,
            buf,
            current_lba,
            num_segments,
            previous_lba: 0,
            progress: None,
            remainder: remainder.try_into().unwrap(),
            sender: None,
            source,
            source_io: None,
            target,
            start_time: None,
        }))
    }

    #[inline]
    fn next_segment(&mut self) -> u32 {
        let num_blocks = if self.num_segments > 0 {
            self.blocks_per_segment
        } else {
            self.num_segments += 1;
            self.remainder
        };

        num_blocks
    }
    /// Copy blocks from source to target with increments of one block (for now).
    /// When the task has been completed, this function returns Ok(true). When a new IO
    /// has been successfully dispatched in returns Ok(false)
    ///
    /// When memory allocation fails, it shall return an error no attempts will be made to
    /// restart a build automatically, ideally we want this to be done from the control plane
    /// and not internally
    ///
    pub fn dispatch_next_segment(&mut self) -> Result<bool, Error> {
        let next_segment = self.next_segment();
        if next_segment == 0 {
            self.shutdown(true);
            return Ok(true);
        }

        if self.current_lba < self.source.get_bdev().num_blocks() {
            let ret = unsafe {
                spdk_bdev_read_blocks(
                    self.source.desc,
                    self.source.ch,
                    self.buf.buf,
                    self.current_lba,
                    next_segment as u64,
                    Some(Self::read_complete),
                    &*self as *const _ as *mut _,
                )
            };

            if ret == 0 {
                self.current_lba += next_segment as u64;
                self.num_segments -= 1;
                Ok(false)
            } else {
                /// we should be able to retry later for now fail on all errors; typically with
                /// ENOMEM we should retry however, we want to delay this so likely use a
                /// (one time) poller?
                Err(Error::Internal("failed to dispatch IO".into()))
            }
        } else {
            assert_eq!(self.current_lba, self.source.get_bdev().num_blocks());
            trace!("Rebuild task completed! \\o/");
            Ok(true)
        }
    }

    /// stop the progress poller
    fn stop_progress(&mut self) {
        if let Some(mut poller) = self.progress.take() {
            unsafe {
                spdk_poller_unregister(&mut poller);
            }
        }
    }

    /// send the waiter that we completed successfully
    fn send_completion(&mut self, success: bool) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(success);
        }
    }

    extern "C" fn progress(copy_task: *mut c_void) -> i32 {
        let mut task = unsafe { Box::from_raw(copy_task as *mut RebuildTask) };
        info!(
            "Rebuild from {} to {} MiBs: {}",
            task.source.get_bdev().name(),
            task.target.get_bdev().name(),
            ((task.current_lba - task.previous_lba) * task.source.get_bdev().block_len() as u64 >> 20) * 2
        );

        task.previous_lba = task.current_lba;

        std::mem::forget(task);
        0
    }

    /// the actual start rebuild task in FFI context
    extern "C" fn _start_rebuild(copy_task: *mut c_void, _arg2: *mut c_void) {
        let mut task = unsafe { Box::from_raw(copy_task as *mut RebuildTask) };
        task.start_time = Some(std::time::SystemTime::now());
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

    pub fn start_rebuild(
        mut task: Box<RebuildTask>,
    ) -> Option<oneshot::Receiver<bool>> {
        let current_core = unsafe { spdk_env_get_current_core() };
        trace!("Will start rebuild task on core {}", current_core);
        trace!("rebuild started at: {:?}", std::time::SystemTime::now());

        let (s, r) = oneshot::channel::<bool>();
        task.sender = Some(s);

        let poller = unsafe {
            spdk_poller_register(
                Some(Self::progress),
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
                current_core,
                Some(Self::_start_rebuild),
                Box::into_raw(task) as *mut c_void,
                std::ptr::null_mut(),
            )
        };

        if event.is_null() {
            return None;
        }

        unsafe {
            spdk_event_call(event);
        }

        Some(r)
    }
}
