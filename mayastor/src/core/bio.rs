use core::fmt;
use std::{
    fmt::{Debug, Formatter},
    ptr::NonNull,
};

use libc::c_void;

use spdk_sys::{
    spdk_bdev_free_io,
    spdk_bdev_io,
    spdk_bdev_io_complete,
    spdk_bdev_io_get_io_channel,
};

use crate::{
    bdev::{
        nexus::{
            nexus_bdev::{Nexus, NEXUS_PRODUCT_ID},
            nexus_fn_table::NexusFnTable,
        },
    },
    core::{Bdev, Cores, GenericStatusCode, Mthread, NvmeStatus, IoChannel, Reactors},
    nexus_uri::bdev_destroy,
};



#[derive(Debug, Copy, Clone, PartialOrd, PartialEq, Eq)]
pub enum IoType {
    Invalid,
    Read,
    Write,
    Unmap,
    Flush,
    Reset,
    NvmeAdmin,
    NvmeIo,
    NvmeIoMd,
    WriteZeros,
    ZeroCopy,
    ZoneInfo,
    ZoneManagement,
    ZoneAppend,
    Compare,
    CompareAndWrite,
    Abort,
    IoNumTypes,
}

#[derive(Debug, Copy, Clone, PartialOrd, PartialEq, Eq)]
#[non_exhaustive]
pub enum IoStatus {
    Aborted,
    FirstFusedFailed,
    MisCompared,
    NoMemory,
    ScsiError,
    NvmeError,
    Failed,
    Pending,
    Success,
}

impl From<IoType> for u32 {
    fn from(t: IoType) -> Self {
        match t {
            IoType::Invalid => 0,
            IoType::Read => 1,
            IoType::Write => 2,
            IoType::Unmap => 3,
            IoType::Flush => 4,
            IoType::Reset => 5,
            IoType::NvmeAdmin => 6,
            IoType::NvmeIo => 7,
            IoType::NvmeIoMd => 8,
            IoType::WriteZeros => 9,
            IoType::ZeroCopy => 10,
            IoType::ZoneInfo => 11,
            IoType::ZoneManagement => 12,
            IoType::ZoneAppend => 13,
            IoType::Compare => 14,
            IoType::CompareAndWrite => 15,
            IoType::Abort => 16,
            IoType::IoNumTypes => 17,
        }
    }
}

impl From<u32> for IoType {
    fn from(u: u32) -> Self {
        match u {
            0 => Self::Invalid,
            1 => Self::Read,
            2 => Self::Write,
            3 => Self::Unmap,
            4 => Self::Flush,
            5 => Self::Reset,
            6 => Self::NvmeAdmin,
            7 => Self::NvmeIo,
            8 => Self::NvmeIoMd,
            9 => Self::WriteZeros,
            10 => Self::ZeroCopy,
            11 => Self::ZoneInfo,
            12 => Self::ZoneManagement,
            13 => Self::ZoneAppend,
            14 => Self::Compare,
            15 => Self::CompareAndWrite,
            16 => Self::Abort,
            17 => Self::IoNumTypes,
            _ => panic!("invalid IO type"),
        }
    }
}

impl From<i32> for IoStatus {
    fn from(status: i32) -> Self {
        match status {
            -7 => Self::Aborted,
            -6 => Self::FirstFusedFailed,
            -5 => Self::MisCompared,
            -4 => Self::NoMemory,
            -3 => Self::ScsiError,
            -2 => Self::NvmeError,
            -1 => Self::Failed,
            0 => Self::Pending,
            1 => Self::Success,
            _ => panic!("invalid status code"),
        }
    }
}

impl From<IoStatus> for i32 {
    fn from(i: IoStatus) -> Self {
        match i {
            IoStatus::Aborted => -7,
            IoStatus::FirstFusedFailed => -6,
            IoStatus::MisCompared => -5,
            IoStatus::NoMemory => -4,
            IoStatus::ScsiError => -3,
            IoStatus::NvmeError => -2,
            IoStatus::Failed => -1,
            IoStatus::Pending => 0,
            IoStatus::Success => 1,
        }
    }
}

impl From<i8> for IoStatus {
    fn from(status: i8) -> Self {
        (status as i32).into()
    }
}

#[derive(Clone)]
pub struct Bio(NonNull<spdk_bdev_io>);

impl From<*mut c_void> for Bio {
    fn from(io: *mut c_void) -> Self {
        Bio(NonNull::new(io as *mut spdk_bdev_io).expect("null ptr"))
    }
}

impl From<*mut spdk_bdev_io> for Bio {
    fn from(io: *mut spdk_bdev_io) -> Self {
        Bio(NonNull::new(io as *mut spdk_bdev_io).expect("null ptr"))
    }
}

impl Bio {
    /// obtain tbe Bdev this IO is associated with
    pub(crate) fn bdev_as_ref(&self) -> Bdev {
        unsafe { Bdev::from(self.0.as_ref().bdev) }
    }

    pub(crate) fn io_channel(&self) -> IoChannel {
        IoChannel::from(unsafe { spdk_bdev_io_get_io_channel(self.0.as_ptr()) })
    }

    // /// initialize the ctx fields of an spdk_bdev_io
    // pub fn init(&mut self) {
    // }

    /// reset the ctx fields of an spdk_bdev_io to submit or resubmit an IO
    // pub fn reset(&mut self, in_flight: usize) {
    //     self.ctx_as_mut_ref().in_flight = in_flight as i8;
    //     self.ctx_as_mut_ref().status = IoStatus::Success;
    // }

    /// complete an IO for the nexus. In the IO completion routine in
    /// `[nexus_bdev]` will set the IoStatus for each IO where success ==
    /// false.
    #[inline]
    pub(crate) fn ok(&mut self) {
        // if cfg!(debug_assertions) {
        //     // have a child IO that has failed
        //     if self.ctx_as_mut_ref().status != IoStatus::Success {
        //         debug!("BIO for nexus {} failed", self.nexus_as_ref().name)
        //     }
        //     // we are marking the IO done but not all child IOs have returned,
        //     // regardless of their state at this point
        //     if self.ctx_as_mut_ref().in_flight != 0 {
        //         debug!("BIO for nexus marked completed but has outstanding")
        //     }
        // }
        unsafe {
            spdk_bdev_io_complete(self.0.as_ptr(), IoStatus::Success.into())
        }
    }
    /// mark the IO as failed
    #[inline]
    pub(crate) fn fail(&self) {
        unsafe {
            spdk_bdev_io_complete(self.0.as_ptr(), IoStatus::Failed.into())
        }
    }

    #[inline]
    pub(crate) fn complete(&mut self) {
        // let pio_ctx = self.ctx_as_mut_ref();
        // if pio_ctx.in_flight == 0 {
        //     if pio_ctx.status == IoStatus::Failed {
        //         pio_ctx.io_attempts -= 1;
        //         if pio_ctx.io_attempts > 0 {
        //             NexusFnTable::nexus_submit(
        //                 IoChannel::from(self.io_channel()),
        //                 &mut self.clone(),
        //             );
        //         } else {
        //             self.fail();
        //         }
        //     } else {
                self.ok();
        //     }
        // }
    }

    /// assess the IO if we need to mark it failed or ok.
    #[inline]
    pub(crate) fn assess(&mut self, child_io: &mut Bio, success: bool) {
        // self.ctx_as_mut_ref().dec();
        //
        // if !success {
        //     // currently, only tests send those but invalid op codes should not
        //     // result into faulting a child device.
        //     if NvmeStatus::from(child_io.clone()).status_code()
        //         == GenericStatusCode::InvalidOpcode
        //     {
        //         self.complete();
        //         return;
        //     }
        //
        //     // // all other status codes indicate a fatal error
        //     // Reactors::master().send_future(Self::child_retire(
        //     //     self.nexus_as_ref().name.clone(),
        //     //     child_io.bdev_as_ref(),
        //     // ));
        // }

        self.complete();
    }


    /// obtain the Nexus struct embedded within the bdev
    pub(crate) fn nexus_as_ref(&self) -> &Nexus {
        let b = self.bdev_as_ref();
        assert_eq!(b.product_name(), NEXUS_PRODUCT_ID);
        unsafe { Nexus::from_raw((*b.as_ptr()).ctxt) }
    }

    /// get the context specifics  of the given IO
    #[inline]
    pub(crate) fn specific<T>(&mut self) -> &mut T {
        unsafe {
            &mut *(self.0.as_mut().driver_ctx.as_mut_ptr() as *mut T)
        }
    }

    /// get a raw pointer to the base of the iov
    #[inline]
    pub(crate) fn iovs(&self) -> *mut spdk_sys::iovec {
        unsafe { self.0.as_ref().u.bdev.iovs }
    }

    /// number of iovs that are part of this IO
    #[inline]
    pub(crate) fn iov_count(&self) -> i32 {
        unsafe { self.0.as_ref().u.bdev.iovcnt }
    }

    /// offset where we do the IO on the device
    #[inline]
    pub(crate) fn offset(&self) -> u64 {
        unsafe { self.0.as_ref().u.bdev.offset_blocks }
    }

    /// num of blocks this IO will read/write/unmap
    #[inline]
    pub(crate) fn num_blocks(&self) -> u64 {
        unsafe { self.0.as_ref().u.bdev.num_blocks }
    }

    /// NVMe passthru command
    #[inline]
    pub(crate) fn nvme_cmd(&self) -> spdk_sys::spdk_nvme_cmd {
        unsafe { self.0.as_ref().u.nvme_passthru.cmd }
    }

    /// raw pointer to NVMe passthru data buffer
    #[inline]
    pub(crate) fn nvme_buf(&self) -> *mut c_void {
        unsafe { self.0.as_ref().u.nvme_passthru.buf as *mut _ }
    }

    /// NVMe passthru number of bytes to transfer
    #[inline]
    pub(crate) fn nvme_nbytes(&self) -> u64 {
        unsafe { self.0.as_ref().u.nvme_passthru.nbytes }
    }

    /// free the IO
    pub(crate) fn free(&self) {
        unsafe { spdk_bdev_free_io(self.0.as_ptr()) }
    }

    /// determine the type of this IO
    #[inline]
    pub(crate) fn io_type(&self) -> IoType {
        unsafe { self.0.as_ref().type_ as u32 }.into()
    }

    /// get the block length of this IO
    #[inline]
    pub(crate) fn block_len(&self) -> u64 {
        self.bdev_as_ref().block_len() as u64
    }
    #[inline]
    pub(crate) fn status(&self) -> IoStatus {
        unsafe { self.0.as_ref().internal.status }.into()
    }

    /// determine if the IO needs an indirect buffer this can happen for example
    /// when we do a 512 write to a 4k device.
    #[inline]
    pub(crate) fn need_buf(&self) -> bool {
        unsafe {
            let slice = std::slice::from_raw_parts_mut(
                self.iovs(),
                self.iov_count() as usize,
            );

            slice[0].iov_base.is_null()
        }
    }

    pub(crate) fn as_ptr(&self) -> *mut spdk_bdev_io {
        self.0.as_ptr()
    }
}

impl Debug for Bio {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "bdev: {} offset: {:?}, num_blocks: {:?}, type: {:?} status: {:?}, {:p} ",
            self.bdev_as_ref().name(),
            self.offset(),
            self.num_blocks(),
            self.io_type(),
            self.status(),
            self
        )
    }
}
