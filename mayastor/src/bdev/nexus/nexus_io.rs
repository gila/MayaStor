use std::{
    fmt::{Debug},
};

use libc::c_void;

use spdk_sys::{
    spdk_bdev_io_complete,
    spdk_bdev_io_get_io_channel,
    bdev_io_from_ctx,
};

use crate::{
    core::Bio,
    bdev::{
        nexus::{
            nexus_channel::DrEvent,
        },
        nexus_lookup,
        ChildState,
        NexusStatus,
        Reason,
    },
    core::{Bdev, Cores, GenericStatusCode, Mthread, NvmeStatus, IoChannel, Reactors},
    nexus_uri::bdev_destroy,
};
use crate::core::IoStatus;

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
/// NioCtx provides context on a per IO basis
#[derive(Debug, Clone)]
pub struct NioCtx {
    /// read consistency
    pub(crate) in_flight: u8,
    /// status of the IO
    disposition: Disposition,
    /// attempts left
    pub(crate) io_attempts: u8,
    /// number of IOs needed
    io_needed: u8,
   /// killme
    pub (crate) status: IoStatus,
}

#[derive(Debug, Clone)]
#[repr(C)]
enum Disposition {
    Complete,
    Failed,
}

impl NioCtx {
    #[inline]
    pub fn dec(&mut self) {
        self.in_flight -= 1;
        debug_assert!(self.in_flight >= 0);
    }

    pub fn setup(&mut self) {

    }

    fn as_io(&self) -> *mut c_void  {
       let ptr=  self as *const _ as *mut _;
        dbg!(ptr);
        ptr
       //*self as *const _ as *mut _
    }


    fn disposition() -> Disposition {
       Disposition::Failed
    }

    fn bio(&self) -> Bio {
      let ptr = self as *const _;
      let bio = container_of!(ptr, spdk_sys::spdk_bdev_io, driver_ctx);
      let b = Bio::from(bio);
      dbg!(&b);
      b
    }

    pub fn complete(&self, status: IoStatus) {
        self.bio().fail();
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
