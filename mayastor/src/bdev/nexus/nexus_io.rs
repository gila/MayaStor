use std::{
    fmt::Debug,
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

use libc::c_void;
use nix::errno::Errno;

use spdk_sys::{spdk_bdev_io, spdk_bdev_io_get_buf, spdk_io_channel};

use crate::{
    bdev::{
        device_destroy,
        nexus::{
            nexus_bdev::NEXUS_PRODUCT_ID,
            nexus_channel::{DrEvent, NexusChannel, NexusChannelInner},
        },
        nexus_lookup,
        ChildState,
        Nexus,
        NexusState,
        NexusStatus,
        Reason,
    },
    core::{
        Bio,
        BlockDevice,
        BlockDeviceHandle,
        CoreError,
        Cores,
        GenericStatusCode,
        IoCompletionStatus,
        IoStatus,
        IoType,
        Mthread,
        NvmeCommandStatus,
        Reactor,
        Reactors,
    },
};

#[allow(unused_macros)]
macro_rules! offset_of {
    ($container:ty, $field:ident) => {
        unsafe { &(*(0usize as *const $container)).$field as *const _ as usize }
    };
}

#[allow(unused_macros)]
macro_rules! container_of {
    ($ptr:ident, $container:ty, $field:ident) => {{
        (($ptr as usize) - offset_of!($container, $field)) as *mut $container
    }};
}

#[repr(transparent)]
#[derive(Debug, Clone)]
pub(crate) struct NexusBio(Bio);

impl DerefMut for NexusBio {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Deref for NexusBio {
    type Target = Bio;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<*mut c_void> for NexusBio {
    fn from(ptr: *mut c_void) -> Self {
        Self(Bio::from(ptr))
    }
}

impl From<*mut spdk_bdev_io> for NexusBio {
    fn from(ptr: *mut spdk_bdev_io) -> Self {
        Self(Bio::from(ptr))
    }
}

impl NexusBio {

    fn as_ptr(&self) -> *mut spdk_bdev_io {
        self.0.as_ptr()
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct NioCtx {
    pub in_flight: u8,
    num_ok: u8,
    status: IoStatus,
    channel: NonNull<spdk_io_channel>,
    core: u32,
    must_fail: bool,
    submission_failure: bool,
}

#[derive(Debug, Clone)]
#[repr(C)]
enum Disposition {
    /// All IOs are completed, and the final status of the IO should be set to
    /// the enum variant
    Complete(IoStatus),
    /// IOs are still in flight status of the last IO that failed
    Flying(IoStatus),
    /// retire the current child
    Retire(IoStatus),
}

pub(crate) fn nexus_submit_io(mut io: NexusBio) {
    // Fail-fast all incoming I/O if the flag is set.
    let inner = io.inner_channel();
    if inner.fail_fast > 0 {
        return;
    }

    if let Err(e) = match io.cmd() {
        IoType::Read => io.readv(),
        // these IOs are submitted to all the underlying children
        IoType::Write | IoType::WriteZeros | IoType::Reset | IoType::Unmap => {
            io.submit_all()
        }
        IoType::Flush => {
            io.ok();
            Ok(())
        }
        IoType::NvmeAdmin => {
            io.fail();
            Err(CoreError::NotSupported {
                source: Errno::EINVAL,
            })
        }
        _ => {
            trace!(?io, "not supported");
            io.fail();
            Err(CoreError::NotSupported {
                source: Errno::EOPNOTSUPP,
            })
        }
    } {
        error!(?e, ?io, "Error during IO submission");
    }
}

impl NexusBio {
    /// helper function to wrap the raw pointers into new types. From here we
    /// should not be dealing with any raw pointers.
    pub unsafe fn nexus_bio_setup(
        channel: *mut spdk_sys::spdk_io_channel,
        io: *mut spdk_sys::spdk_bdev_io,
    ) -> Self {
        let mut bio = NexusBio::from(io);
        let ctx = bio.ctx_as_mut();
        // for verification purposes when retiring a child
        ctx.core = Cores::current();
        ctx.channel = NonNull::new(channel).unwrap();
        ctx.status = IoStatus::Pending;
        ctx.in_flight = 0;
        ctx.num_ok = 0;
        ctx.must_fail = false;
        ctx.submission_failure = false;
        bio
    }

    /// invoked when a nexus Io completes
    fn child_completion(
        device: &dyn BlockDevice,
        status: IoCompletionStatus,
        ctx: *mut c_void,
    ) {
        let mut nexus_io = NexusBio::from(ctx as *mut spdk_bdev_io);
        nexus_io.complete(device, status);
    }

    #[inline(always)]
    /// a mutable reference to the IO context
    pub fn ctx_as_mut(&mut self) -> &mut NioCtx {
        self.specific_as_mut::<NioCtx>()
    }

    #[inline(always)]
    /// immutable reference to the IO context
    pub fn ctx(&self) -> &NioCtx {
        self.specific::<NioCtx>()
    }

    /// returns the type of command for this IO
    #[inline(always)]
    fn cmd(&self) -> IoType {
        self.io_type()
    }

    /// completion handler for the nexus when a child IO completes
    pub fn complete(
        &mut self,
        child: &dyn BlockDevice,
        status: IoCompletionStatus,
    ) {
        assert_eq!(self.ctx().core, Cores::current());
        let success = status == IoCompletionStatus::Success;

        // decrement the counter of in flight IO
        self.ctx_as_mut().in_flight -= 1;

        // record the state of at least one of the IO's.
        if success {
            self.ctx_as_mut().num_ok += 1;
            return self.ok_checked();
        } else {
            // IO failure, mark the IO failed and taka the child out
            info!(?self, "{} failed IO", child.device_name());
            self.ctx_as_mut().status = IoStatus::Failed;
            self.ctx_as_mut().must_fail = true;
            return self.handle_failure(child, status);
        }
    }

    fn ok_checked(&mut self) {
        if self.ctx().in_flight == 0 {
            if self.ctx().submission_failure {
                self.fail();
            } else {
                self.ok();
            }
        }
    }

    pub fn fail_checked(&mut self) {
        if self.ctx().in_flight == 0 {
            self.fail();
        }
    }

    pub fn retry_checked(&mut self) {
        if self.ctx().in_flight == 0 {
            let bio = unsafe { Self::nexus_bio_setup( self.ctx().channel.as_ptr(), self.as_ptr()) };
            nexus_submit_io(bio);
        }
    }

    /// reference to the inner channels. The inner channel contains the specific
    /// per-core data structures.
    #[allow(clippy::mut_from_ref)]
    fn inner_channel(&self) -> &mut NexusChannelInner {
        NexusChannel::inner_from_channel(self.ctx().channel.as_ptr())
    }

    //TODO make const
    fn data_ent_offset(&self) -> u64 {
        let b = self.bdev();
        assert_eq!(b.product_name(), NEXUS_PRODUCT_ID);
        unsafe { Nexus::from_raw((*b.as_ptr()).ctxt) }.data_ent_offset
    }

    /// helper routine to get a channel to read from
    fn read_channel_at_index(&self, i: usize) -> &dyn BlockDeviceHandle {
        &*self.inner_channel().readers[i]
    }

    /// submit a read operation to one of the children of this nexus
    #[inline(always)]
    fn submit_read(
        &self,
        hdl: &dyn BlockDeviceHandle,
    ) -> Result<(), CoreError> {
        hdl.readv_blocks(
            self.iovs(),
            self.iov_count(),
            self.offset() + self.data_ent_offset(),
            self.num_blocks(),
            Self::child_completion,
            self.as_ptr().cast(),
        )
    }

    fn do_readv(&mut self) -> Result<(), CoreError> {
        let inner = self.inner_channel();

        // Upon buffer allocation we might have been rescheduled, so check
        // the fail-fast flag once more.

        if inner.fail_fast > 0 {
            return Err(CoreError::ReadDispatch {
                source: Errno::ENXIO,
                offset: self.offset(),
                len: self.num_blocks(),
            });
        }

        if let Some(i) = inner.child_select() {
            let hdl = self.read_channel_at_index(i);
            let r = self.submit_read(hdl);

            if r.is_err() {
                // Such a situation can happen when there is no active I/O in
                // the queues, but error on qpair is observed
                // due to network timeout, which initiates
                // controller reset. During controller reset all
                // I/O channels are deinitialized, so no I/O
                // submission is possible (spdk returns -6/ENXIO), so we have to
                // start device retire.
                // TODO: ENOMEM and ENXIO should be handled differently and
                // device should not be retired in case of ENOMEM.
                info!(
                    "{} initiating retire in response to READ submission error",
                    hdl.get_device().device_name(),
                );
                self.do_retire(hdl.get_device().device_name());
            } else {
                self.ctx_as_mut().in_flight += 1;
            }
            r
        } else {
            self.fail();
            Err(CoreError::NoDevicesAvailable {})
        }
    }

    extern "C" fn nexus_get_buf_cb(
        _ch: *mut spdk_io_channel,
        io: *mut spdk_bdev_io,
        success: bool,
    ) {
        let mut bio = NexusBio::from(io);

        if !success {
            error!("Failed to get io buffer for io");
            bio.no_mem();
        } else if let Err(e) = bio.do_readv() {
            error!("Failed to submit I/O after iovec allocation: {:?}", e,);
        }
    }

    /// submit read IO to some child
    fn readv(&mut self) -> Result<(), CoreError> {
        if self.0.need_buf() {
            unsafe {
                spdk_bdev_io_get_buf(
                    self.0.as_ptr(),
                    Some(Self::nexus_get_buf_cb),
                    self.0.num_blocks() * self.0.block_len(),
                )
            }
            Ok(())
        } else {
            self.do_readv()
        }
    }

    #[inline(always)]
    fn submit_write(
        &self,
        hdl: &dyn BlockDeviceHandle,
    ) -> Result<(), CoreError> {
        hdl.writev_blocks(
            self.iovs(),
            self.iov_count(),
            self.offset() + self.data_ent_offset(),
            self.num_blocks(),
            Self::child_completion,
            self.as_ptr().cast(),
        )
    }

    #[inline(always)]
    fn submit_unmap(
        &self,
        hdl: &dyn BlockDeviceHandle,
    ) -> Result<(), CoreError> {
        hdl.unmap_blocks(
            self.offset() + self.data_ent_offset(),
            self.num_blocks(),
            Self::child_completion,
            self.as_ptr().cast(),
        )
    }

    #[inline(always)]
    fn submit_write_zeroes(
        &self,
        hdl: &dyn BlockDeviceHandle,
    ) -> Result<(), CoreError> {
        hdl.write_zeroes(
            self.offset() + self.data_ent_offset(),
            self.num_blocks(),
            Self::child_completion,
            self.as_ptr().cast(),
        )
    }

    #[inline(always)]
    fn submit_reset(
        &self,
        hdl: &dyn BlockDeviceHandle,
    ) -> Result<(), CoreError> {
        hdl.reset(Self::child_completion, self.as_ptr().cast())
    }

    /// Submit the IO to all underlying children, failing on the first error we
    /// find. When an IO is partially submitted -- we must wait until all
    /// the child IOs have completed before we mark the whole IO failed to
    /// avoid double frees. This function handles IO for a subset that must
    /// be submitted to all the underlying children.
    fn submit_all(&mut self) -> Result<(), CoreError> {
        let mut inflight = 0;
        let mut status = IoStatus::Pending;
        // Name of the device which experiences I/O submission failures.
        let mut failed_device = None;

        let cmd = self.cmd();
        let result = self.inner_channel().writers.iter().try_for_each(|h| {
            match cmd {
                IoType::Write => self.submit_write(&**h),
                IoType::Unmap => self.submit_unmap(&**h),
                IoType::WriteZeros => self.submit_write_zeroes(&**h),
                IoType::Reset => self.submit_reset(&**h),
                // we should never reach here, if we do it is a bug.
                _ => unreachable!(),
            }
            .map(|_| {
                inflight += 1;
            })
            .map_err(|se| {
                status = IoStatus::Failed;
                error!(
                    "core: {} thread: {},IO submission  failed with error {:?}, I/Os submitted: {}",
                    Cores::current(), Mthread::current().unwrap().name(), se, inflight
                );

                // Record the name of the device for immediat retire.
                failed_device = Some(h.get_device().device_name());
                se
            })
        });

        // Submission errors can also trigger device retire.
        // Such a situation can happen when there is no active I/O in the
        // queues, but error on qpair is observed due to network
        // timeout, which initiates controller reset. During controller
        // reset all I/O channels are deinitialized, so no I/O
        // submission is possible (spdk returns -6/ENXIO), so we have to
        // start device retire.
        // TODO: ENOMEM and ENXIO should be handled differently and
        // device should not be retired in case of ENOMEM.
        if result.is_err() {
            let device = failed_device.unwrap();
            info!(
                "{}: retiring device in response to submission error={:?}",
                device, result,
            );
            self.ctx_as_mut().submission_failure = true;
        }

        if inflight != 0 {
            self.ctx_as_mut().in_flight = inflight;
        } else {
            // if no IO was submitted at all, we can fail the IO now.
            // TODO: Support of IoStatus::NoMemory in ENOMEM-related errors.
            self.fail();
        }
        result
    }

    fn do_retire(&self, child: String) {
        Reactors::master().send_future(Self::child_retire(
            self.nexus_as_ref().name.clone(),
            child,
        ));
    }

    fn handle_failure(
        &mut self,
        child: &dyn BlockDevice,
        status: IoCompletionStatus,
    ) {
        // we have experienced a failure on one of the child devices. We need to
        // ensure we do not submit more IOs to this child. We do not need to
        // tell other cores about this because they will experience the
        // same errors on their own channels.

        // no new IO can be submitted to this child, it is, however still
        // present and may have pending IO in its queue.
        //

        trace!(?status);

        if let IoCompletionStatus::NvmeError(nvme_status) = status {
            if nvme_status
                == NvmeCommandStatus::GenericCommandStatus(
                    GenericStatusCode::InvalidOpcode,
                )
            {
                info!(
                        "Device {} experienced invalid opcode error: retiring skipped",
                        child.device_name()
                    );
                return;
            }
        }
        let needs_retire =
            self.inner_channel().remove_child(&child.device_name());

        if needs_retire {
            info!("need retire!");
            self.do_retire(child.device_name());
        }

        self.retry_checked();
    }

    fn try_retire(
        &mut self,
        child: &dyn BlockDevice,
        status: IoCompletionStatus,
    ) {
        trace!(?status);

        if let IoCompletionStatus::NvmeError(nvme_status) = status {
            if nvme_status
                == NvmeCommandStatus::GenericCommandStatus(
                    GenericStatusCode::InvalidOpcode,
                )
            {
                info!(
                        "Device {} experienced invalid opcode error: retiring skipped",
                        child.device_name()
                    );
                return;
            }
        }
        info!("{} try_retire() called", child.device_name());
        self.do_retire(child.device_name());
    }

    /// Retire a child for this nexus.
    async fn child_retire(nexus: String, device: String) {
        match nexus_lookup(&nexus) {
            Some(nexus) => {
                warn!(
                    "core {} thread {:?}, faulting child {}",
                    Cores::current(),
                    Mthread::current(),
                    device,
                );

                // Pausing a nexus acts like entering a critical
                // section,
                // allowing only one retire request to run at a
                // time, which prevents
                // inconsistency in reading/updating nexus
                // configuration.
                nexus.pause().await.unwrap();
                nexus.set_failfast().await.unwrap();
                match nexus.child_lookup(&device) {
                    Some(child) => {
                        // TODO: an error can occur here if a
                        // separate task,
                        // e.g. grpc request is also deleting the
                        // child.
                        if let Err(err) = child.destroy().await {
                            error!(
                                "{}: destroying child {} failed {}",
                                nexus, child, err
                            );
                        }
                    }
                    None => {
                        warn!(
                                        "{} no longer belongs to nexus {}, skipping child removal",
                                        device, nexus
                                    );
                    }
                }
                // Lookup child once more and finally remove it.

                nexus.clear_failfast().await.unwrap();
                nexus.resume().await.unwrap();

                if nexus.status() == NexusStatus::Faulted {
                    error!(":{} has no children left... ", nexus);
                }
            }

            None => {
                debug!("{} Nexus does not exist anymore", nexus);
            }
        }
    }
}
