use spdk_sys::{
    spdk_bdev_close,
    spdk_bdev_desc,
    spdk_bdev_desc_get_bdev,
    spdk_bdev_get_io_channel,
    spdk_bdev_module_claim_bdev,
    spdk_bdev_module_release_bdev,
};

use crate::{
    bdev::nexus::nexus_module::NEXUS_MODULE,
    core::{channel::IoChannel, Bdev},
};

/// NewType around a descriptor, only one descriptor is typically available as
/// a bdev is opened only one time. When the last reference to the descriptor is
/// dropped, we implicitly close the bdev.
#[derive(Debug, Clone)]
pub struct Descriptor(*mut spdk_bdev_desc);

impl Drop for Descriptor {
    fn drop(&mut self) {
        debug!("closing bdev {}", self.get_bdev().name());
        unsafe {
            spdk_bdev_close(self.0);
        }
    }
}

impl Descriptor {
    /// returns the underling ptr
    pub fn as_ptr(&self) -> *mut spdk_bdev_desc {
        self.0
    }

    /// Get a channel to the underlying bdev
    pub fn get_channel(&self) -> Option<IoChannel> {
        let ch = unsafe { spdk_bdev_get_io_channel(self.0) };
        if ch.is_null() {
            None
        } else {
            IoChannel::from_null_checked(ch)
        }
    }

    /// claim the bdev for exclusive access, when the descriptor is in read-only
    /// the descriptor will implicitly be upgraded to read/write.
    ///
    /// Not claiming a bdev. Preexisting writers will not be downgraded.
    pub fn claim(&self) -> bool {
        let err = unsafe {
            spdk_bdev_module_claim_bdev(
                self.get_bdev().as_ptr(),
                self.0,
                NEXUS_MODULE.as_ptr(),
            )
        };

        let name = self.get_bdev().name();
        debug!("claimed bdev {}", name);
        err == 0
    }

    /// release a previously claimed bdev
    pub fn release(&self) {
        unsafe {
            if self.get_bdev().is_claimed() {
                spdk_bdev_module_release_bdev(self.get_bdev().as_ptr())
            }
        }
    }

    /// Return the bdev associated with this descriptor, a descriptor can not
    /// exist without a bdev
    pub fn get_bdev(&self) -> Bdev {
        let bdev = unsafe { spdk_bdev_desc_get_bdev(self.0) };
        Bdev::from(bdev)
    }

    /// create a Descriptor from a spdk_bdev_desc pointer this is the only way
    /// to create a new descriptor
    pub fn from_null_checked(desc: *mut spdk_bdev_desc) -> Option<Descriptor> {
        if desc.is_null() {
            None
        } else {
            Some(Descriptor(desc))
        }
    }
}
