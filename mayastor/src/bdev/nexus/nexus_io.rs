use std::{
    fmt::Debug,
};
use crate::ffihelper::{FfiResult, ErrnoResult};
use libc::c_void;

use spdk_sys::{
    spdk_bdev_readv_blocks,
    spdk_bdev_writev_blocks,
    spdk_bdev_io,
};

use crate::{
    bdev::{
        ChildState,
        nexus::{
            nexus_channel::DrEvent,
        },
        nexus_lookup,
        NexusStatus,
        Reason,
    },
    core::{Bdev, Cores, Mthread},
    core::Bio,
    nexus_uri::bdev_destroy,
};

use crate::core::{IoStatus, IoType, BdevHandle};
use crate::bdev::nexus::nexus_channel::{NexusChannel, NexusChannelInner};
use crate::bdev::Nexus;
use crate::bdev::nexus::nexus_bdev::NEXUS_PRODUCT_ID;
use nix::errno::Errno;

macro_rules! offset_of {
    ($container:ty, $field:ident) => (unsafe {
        &(*(0usize as *const $container)).$field as *const _ as usize
    })
}

macro_rules! container_of {
    ($ptr:ident, $container:ty, $field:ident) => ({
        (($ptr as usize) - offset_of!($container, $field)) as *mut $container
    })
}

 #[repr(transparent)]
 pub (crate) struct NexusBio(Bio);

 impl From<*mut c_void>for NexusBio {
     fn from(ptr: *mut c_void) -> Self {
         Self(Bio::from(ptr))
     }
 }

impl From<*mut spdk_bdev_io>for NexusBio {
    fn from(ptr: *mut spdk_bdev_io) -> Self {
        Self(Bio::from(ptr))
    }
}
//type NexusBio = Bio;
/// NioCtx provides context on a per IO basis
#[derive(Debug)]
#[repr(C)]
pub struct NioCtx {
    /// read consistency
    pub(crate) in_flight: u8,
    /// attempts left
    pub(crate) io_attempts: u8,
    /// number of IOs needed
    io_needed: u8,
    /// killme
    pub(crate) status: IoStatus,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
#[repr(C)]
enum Disposition {
    Complete(IoStatus),
    Flying(IoStatus),
    Failed,
}

pub (crate) fn nexus_submit_io(mut io: NexusBio) {

    io.setup();

    match io.cmd() {
        IoType::Read => {
            io.readv().unwrap();
        }
        IoType::Write => {
            io.writev().unwrap();
        }
        _  => { panic!("not implemented") }
    }
}

impl NexusBio {
    unsafe extern "C" fn child_completion(
        child_io: *mut spdk_bdev_io,
        success: bool,
        nexus_io: *mut c_void,
    ) {
        let mut nexus_io = NexusBio::from(nexus_io);
        let child_io = Bio::from(child_io);
        nexus_io.complete(success);
        child_io.free();
    }

    #[inline(always)]
    fn ctx(&mut self) -> &mut NioCtx {
       self.0.specific::<NioCtx>()
    }


    #[inline(always)]
    pub fn setup(&mut self) {
        let ctx = self.ctx();
        ctx.status = IoStatus::Pending;
        ctx.in_flight = 0;
    }

    fn disposition(&mut self) -> Disposition {

        let ctx = self.ctx();
        if ctx.in_flight == 0  && ctx.status == IoStatus::Success {
            return Disposition::Complete(IoStatus::Success)
        }

        if ctx.in_flight == 0 && ctx.status == IoStatus::Failed {
            return Disposition::Complete(IoStatus::Failed)
        }

        Disposition::Flying(IoStatus::Pending)
    }

    #[inline(always)]
    fn bio_ref(&self) -> Bio {
        let ptr = self as *const _;
        let bio = container_of!(ptr, spdk_sys::spdk_bdev_io, driver_ctx);
        Bio::from(bio)
    }

    #[inline(always)]
    fn cmd(&self) -> IoType {
        self.0.io_type()
    }

    pub fn complete(&mut self, success: bool) {
        debug!("completion {:?}", self.0);

        // update counter of in flight IO
        self.ctx().in_flight -=1;

        // record the state of the IO
        if !success {
            self.ctx().status = IoStatus::Failed;
        }

        if self.ctx().in_flight== 0 {

                self.0.ok();

            // if self.ctx().status == IoStatus::Failed {
            //     self.0.fail();
            // }
        }
        //
        // match self.disposition() {
        //     Disposition::Complete(IoStatus::Success) => {
        //         debug!("IO completed inflight is {}", self.ctx().in_flight);
        //         self.0.ok();
        //     },
        //
        //     Disposition::Complete(IoStatus::Failed)  => {
        //         debug!("IO complete but failed");
        //         self.0.fail();
        //     },
        //
        //     Disposition::Flying(IoStatus::Failed) => {
        //         debug!("IO failed but IO in-flight {}", self.ctx().in_flight);
        //     }
        //     Disposition::Flying(_) => {
        //         debug!("IO completed but IO in-flight {}", self.ctx().in_flight);
        //     }
        //     _ => {}
        // }
    }

    fn inner_channel(&self) -> &mut NexusChannelInner {
        let ch = self.0.io_channel();
        NexusChannel::inner_from_channel(ch.as_ptr())
    }

    fn nexus_name(&self) -> String {
        self.0.bdev_as_ref().name()
    }

    fn data_ent_offset(&self) -> u64 {
        let b = self.0.bdev_as_ref();
        assert_eq!(b.product_name(), NEXUS_PRODUCT_ID);
        unsafe { Nexus::from_raw((*b.as_ptr()).ctxt) }.data_ent_offset
    }

    fn read_channel_at_index(&self, i: usize) -> &BdevHandle {
        &self.inner_channel().readers[i]
    }

    fn writers(&self) -> impl Iterator<Item = &BdevHandle> {
        self.inner_channel().writers.iter()
    }

    fn readv(&mut self) -> Result<u8, Errno> {
        let child = self.inner_channel().child_select();
        if child.is_none() {
            error!(
                "no child available to read from {:?}",
                self.0,
            );
            self.0.fail();
            return Err(Errno::ENODEV);
        }

        let (desc, ch) = self.read_channel_at_index(child.unwrap()).io_tuple();

        let ret = unsafe {
            spdk_bdev_readv_blocks(
                desc,
                ch,
                self.0.iovs(),
                self.0.iov_count(),
                self.0.offset() + self.data_ent_offset(),
                self.0.num_blocks(),
                Some(Self::child_completion),
                self.0.as_ptr().cast()
            )
        };

        if ret == 0 {
            self.ctx().in_flight += 1;
            Ok(self.ctx().in_flight)
        } else {
            self.ctx().status = IoStatus::Failed;
            Err(Errno::from_i32(ret.abs()))
        }
    }

    fn writev(&mut self) -> Result<(), Errno> {
        let offset = self.data_ent_offset();
        let mut in_flight : u8 = 0;
        let results = self.inner_channel()
            .writers
            .iter()
            .map(|c| unsafe {
                let (desc, chan) = c.io_tuple();
                spdk_bdev_writev_blocks(
                    desc,
                    chan,
                    self.0.iovs(),
                    self.0.iov_count(),
                    self.0.offset() + self.0.nexus_as_ref().data_ent_offset,
                    self.0.num_blocks(),
                    Some(Self::child_completion),
                    self.0.as_ptr() as *mut _,
                )
            })
            .collect::<Vec<_>>();

        self.ctx().in_flight = self.inner_channel().writers.len() as u8;
        dbg!(self.ctx().in_flight);
        Ok(())

    }
}

async fn child_retire(nexus: String, child: Bdev) {
    error!("{:#?}", child);

    if let Some(nexus) = nexus_lookup(&nexus) {
        if let Some(child) = nexus.child_lookup(&child.name()) {
            let current_state = child.state.compare_and_swap(
                ChildState::Open,
                ChildState::Faulted(Reason::IoError),
            );

            if current_state == ChildState::Open {
                warn!(
                    "core {} thread {:?}, faulting child {}",
                    Cores::current(),
                    Mthread::current(),
                    child,
                );

                let uri = child.name.clone();
                nexus.pause().await.unwrap();
                nexus.reconfigure(DrEvent::ChildFault).await;
                //nexus.remove_child(&uri).await.unwrap();

                // Note, an error can occur here if a separate task,
                // e.g. grpc request is also deleting the child,
                // in which case the bdev may no longer exist at
                // this point. To be addressed by CAS-632 to
                // improve synchronization.
                if let Err(err) = bdev_destroy(&uri).await {
                    error!("{} destroying bdev {}", err, uri)
                }

                nexus.resume().await.unwrap();
                if nexus.status() == NexusStatus::Faulted {
                    error!(":{} has no children left... ", nexus);
                }
            }
        }
    } else {
        debug!("{} does not belong (anymore) to nexus {}", child, nexus);
    }
}

/// NVMe Admin opcode, from nvme_spec.h
pub mod nvme_admin_opc {
    // pub const GET_LOG_PAGE: u8 = 0x02;
    pub const IDENTIFY: u8 = 0x06;
    // pub const ABORT: u8 = 0x08;
    // pub const SET_FEATURES: u8 = 0x09;
    // pub const GET_FEATURES: u8 = 0x0a;
    // Vendor-specific
    pub const CREATE_SNAPSHOT: u8 = 0xc0;
}
