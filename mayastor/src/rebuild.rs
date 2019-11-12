use std::os::raw::c_void;

use spdk_sys::{
    spdk_app_stop, spdk_bdev_desc, spdk_bdev_free_io, spdk_bdev_get_io_channel,
    spdk_bdev_io, spdk_bdev_read, spdk_bdev_read_blocks,
    spdk_bdev_write_blocks, spdk_bdev_writev, spdk_bdev_writev_blocks,
    spdk_env_get_current_core, spdk_event_allocate, spdk_event_call,
};

use crate::bdev::nexus::nexus_io::Bio;
use crate::bdev::nexus::Error;
use crate::descriptor::Descriptor;

/// struct that holds the state of a copy task. This struct
/// is used during rebuild.
#[derive(Debug)]
pub struct CopyTask {
    /// the source where to copy from
    source: Descriptor,
    /// the target where to copy to
    target: Descriptor,
    /// the last LBA for which an io copy has been submitted
    current_lba: u64,
    source_io: Option<*mut spdk_bdev_io>,
    // queue IO to dispatch during next poller run
    // queue: VecDeque<DmaBuf>,
}

impl CopyTask {
    extern "C" fn write_complete(
        io: *mut spdk_bdev_io,
        success: bool,
        arg: *mut c_void,
    ) {
        let mut task = unsafe { Box::from_raw(arg as *mut CopyTask) };
        let source_io = task.source_io.take().unwrap();
        unsafe {
            spdk_bdev_free_io(io);
            spdk_bdev_free_io(source_io);
        }

        if let Ok(next) = task.dispatch_next_block() {
            if next == true {
                info!(
                    "Rebuild completed at {:?} from {} to {} completed successfully!",
                    std::time::SystemTime::now(),
                    task.source.get_bdev().name(),
                    task.target.get_bdev().name()
                );
                std::mem::drop(task);
                unsafe {
                    spdk_app_stop(0);
                }
            } else {
                std::mem::forget(task);
            }
        } else {
            panic!("error in rebuild");
        }
    }

    extern "C" fn read_complete(
        io: *mut spdk_bdev_io,
        success: bool,
        arg: *mut c_void,
    ) {
        let mut task = unsafe { Box::from_raw(arg as *mut CopyTask) };
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
            warn!("rebuild IO failed!");
            unsafe {
                spdk_bdev_free_io(io);
            }
        }
    }

    /// returns a Box<CopyTask>
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
        }))
    }

    /// Copy blocks from source to target with increments of one block (for now).
    /// When the task has been completed, this function returns Ok(true). When a new IO
    /// has been successfully dispatched in returns Ok(false)
    ///
    /// When memory allocation fails, it shall return an error
    pub fn dispatch_next_block(&mut self) -> Result<bool, Error> {
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
                dbg!(ret);
                warn!("failed to dispatch rebuild IO, will retry");
                Ok(false)
            }
        } else {
            assert_eq!(self.current_lba, self.source.get_bdev().num_blocks());
            trace!("rebuild task completed! \\o/");
            Ok(true)
        }
    }

    extern "C" fn rebuild_task(copy_task: *mut c_void, arg2: *mut c_void) {
        let mut task = unsafe { Box::from_raw(copy_task as *mut CopyTask) };
        let next = task.dispatch_next_block();
        std::mem::forget(task);
    }

    pub fn start_rebuild(task: Box<CopyTask>) {
        let current_core = unsafe { spdk_env_get_current_core() };
        trace!("Will start rebuild task on core {}", current_core);
        trace!("rebuild started at: {:?}", std::time::SystemTime::now());

        let event = unsafe {
            spdk_event_allocate(
                current_core,
                Some(Self::rebuild_task),
                Box::into_raw(task) as *mut c_void,
                std::ptr::null_mut(),
            )
        };

        unsafe {
            spdk_event_call(event);
        }
    }
}
