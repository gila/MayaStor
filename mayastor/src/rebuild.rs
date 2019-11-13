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

/// struct that holds the state of a copy task. This struct
/// is used during rebuild.
#[derive(Debug)]
pub struct RebuildTask {
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
}

impl RebuildTask {
    extern "C" fn write_complete(
        io: *mut spdk_bdev_io,
        _success: bool,
        arg: *mut c_void,
    ) {
        let mut task = unsafe { Box::from_raw(arg as *mut RebuildTask) };

        let sio = Bio::from(task.source_io.take().unwrap());
        let bio = Bio::from(io);

        assert_eq!(bio.offset(), sio.offset());
        assert_eq!(bio.num_blocks(), sio.num_blocks());

        Bio::io_free(io);
        Bio::io_free(sio.io);

        match task.dispatch_next_segment() {
            Ok(next) => {
                if next {
                    info!(
                    "Rebuild completed at {:?} from {} to {} completed successfully!",
                    std::time::SystemTime::now(),
                    task.source.get_bdev().name(),
                    task.target.get_bdev().name());

                    task.stop_progress();
                    task.send_completion();
                } else {
                    // we are not done yet, forget the task to avoid dropping
                    std::mem::forget(task);
                }
            }
            Err(..) => {
                // task will be dropped
                panic!("error in rebuild");
            }
        }
    }

    /// callback when the read of the rebuild progress has completed
    extern "C" fn read_complete(
        io: *mut spdk_bdev_io,
        success: bool,
        arg: *mut c_void,
    ) {
        let mut task = unsafe { Box::from_raw(arg as *mut RebuildTask) };
        if success {
            unsafe {
                let bio = Bio::from(io);
                let desc = task.target.desc;
                let ch = task.target.ch;
                task.source_io = Some(io);

                spdk_bdev_writev_blocks(
                    desc,
                    ch,
                    bio.iovs(),
                    bio.iov_count(),
                    bio.offset(),
                    bio.num_blocks(),
                    Some(Self::write_complete),
                    Box::into_raw(task) as *const _ as *mut _,
                );
            }
        } else {
            // the task will be dropped here the receiver would be cancelled too
            warn!("rebuild IO failed!");
            unsafe {
                spdk_bdev_free_io(io);
            }
        }
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

        Ok(Box::new(Self {
            source,
            target,
            current_lba,
            source_io: None,
            sender: None,
            progress: None,
        }))
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
        if self.current_lba < self.source.get_bdev().num_blocks() {
            let ret = unsafe {
                spdk_bdev_read_blocks(
                    self.source.desc,
                    self.source.ch,
                    std::ptr::null_mut(),
                    self.current_lba,
                    1,
                    Some(Self::read_complete),
                    &*self as *const _ as *mut _,
                )
            };

            // if we failed to dispatch the IO, we will redo it later return Ok(false)
            if ret == 0 {
                self.current_lba += 1;
                Ok(false)
            } else {
                // for now fail on all errors; typically with ENOMEM we should retry
                // however, we want to delay this so likely use a (one time) poller?
                Err(Error::Internal("failed to dispatch IO".into()))
            }
        } else {
            assert_eq!(self.current_lba, self.source.get_bdev().num_blocks());
            trace!("rebuild task completed! \\o/");
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
    fn send_completion(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(true);
        }
    }

    extern "C" fn progress(copy_task: *mut c_void) -> i32 {
        let task = unsafe { Box::from_raw(copy_task as *mut RebuildTask) };
        info!(
            "Rebuild from {} to {} with blocks to go: {}",
            task.source.get_bdev().name(),
            task.target.get_bdev().name(),
            task.source.get_bdev().num_blocks() - task.current_lba
        );
        std::mem::forget(task);
        0
    }

    /// the actual start rebuild task in FFI context
    extern "C" fn _start_rebuild(copy_task: *mut c_void, _arg2: *mut c_void) {
        let mut task = unsafe { Box::from_raw(copy_task as *mut RebuildTask) };

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

        // cant fail?
        unsafe {
            // the event will be put back into the mem pool when the reactor de-queues it
            spdk_event_call(event);
        }

        Some(r)
    }
}
