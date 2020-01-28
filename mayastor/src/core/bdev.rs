use std::{
    ffi::CStr,
    fmt::{Debug, Formatter},
    os::raw::c_void,
};

use spdk_sys::{
    spdk_bdev,
    spdk_bdev_get_aliases,
    spdk_bdev_get_block_size,
    spdk_bdev_get_by_name,
    spdk_bdev_get_name,
    spdk_bdev_get_num_blocks,
    spdk_bdev_get_product_name,
    spdk_bdev_get_uuid,
    spdk_bdev_io_type_supported,
    spdk_bdev_open,
    spdk_uuid_generate,
};

use crate::core::{uuid::Uuid, Descriptor};
// XXX FIX THIS
use crate::descriptor::DescError;
use nix::errno::Errno;

/// Newtype structure that represents a block device. The soundness of the API
/// is based on the fact that opening and finding of a bdev, returns a valid
/// bdev or None. Once the bdev is given, the operations on the bdev are safe.
/// It is not possible to remove a bdev through a core other than the management
/// core. This means that the structure is always valid for the lifetime of the
/// scope.
pub struct Bdev(*mut spdk_bdev);

impl From<*mut spdk_bdev> for Bdev {
    fn from(b: *mut spdk_bdev) -> Self {
        Self(b)
    }
}
impl Debug for Bdev {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        writeln!(
            f,
            "name: {}, driver: {}, product: {}, num_blocks: {}, block_len {}",
            self.name(),
            self.driver(),
            self.product_name(),
            self.num_blocks(),
            self.block_len()
        )
    }
}

impl Bdev {
    extern "C" fn hot_remove(ctx: *mut c_void) {
        let bdev = Bdev(ctx as *mut _);
        debug!("called hot remove cb for nexus {:?}", bdev);
    }
    /// open a bdev by its name in read_write mode. If a bdev is claimed, it can
    /// be opened for write only once.
    pub fn open(name: &str, read_write: bool) -> Result<Descriptor, DescError> {
        if let Some(bdev) = Self::lookup_by_name(name) {
            let mut descriptor = std::ptr::null_mut();

            let rc = unsafe {
                spdk_bdev_open(
                    bdev.as_ptr(),
                    read_write,
                    Some(Self::hot_remove),
                    bdev.as_ptr() as *mut _,
                    &mut descriptor,
                )
            };

            return if rc != 0 {
                Err(DescError::OpenBdev {
                    source: Errno::from_i32(rc),
                })
            } else {
                Ok(Descriptor::from_null_checked(descriptor).unwrap())
            };
        }

        Err(DescError::OpenBdev {
            source: Errno::ENODEV,
        })
    }

    pub fn is_claimed(&self) -> bool {
        unsafe { !(*self.0).internal.claim_module.is_null() }
    }

    /// lookup a bdev by its name
    pub fn lookup_by_name(name: &str) -> Option<Bdev> {
        let name = std::ffi::CString::new(name).unwrap();

        let bdev = unsafe { spdk_bdev_get_by_name(name.as_ptr()) };
        if bdev.is_null() {
            None
        } else {
            Some(Bdev(bdev))
        }
    }
    /// returns the block_size of the underlying device
    pub fn block_len(&self) -> u32 {
        unsafe { spdk_bdev_get_block_size(self.0) }
    }

    /// number of blocks for this device
    pub fn num_blocks(&self) -> u64 {
        unsafe { spdk_bdev_get_num_blocks(self.0) }
    }

    /// set the block count of this device
    pub fn set_block_count(&self, count: u64) {
        unsafe {
            (*self.0).blockcnt = count;
        }
    }

    /// set the block length of the device in bytes
    pub fn set_block_len(&self, len: u32) {
        unsafe {
            (*self.0).blocklen = len;
        }
    }

    /// return the bdev size in bytes
    pub fn size_in_bytes(&self) -> u64 {
        self.num_blocks() * self.block_len() as u64
    }

    /// returns the alignment of the bdev
    pub fn alignment(&self) -> u8 {
        unsafe { (*self.0).required_alignment }
    }

    /// returns the configured product name
    pub fn product_name(&self) -> String {
        unsafe {
            CStr::from_ptr(spdk_bdev_get_product_name(self.0))
                .to_str()
                .unwrap()
                .to_string()
        }
    }

    /// returns the name of driver module for the given bdev
    pub fn driver(&self) -> String {
        unsafe {
            CStr::from_ptr((*(*self.0).module).name)
                .to_str()
                .unwrap()
                .to_string()
        }
    }

    /// returns the bdev name
    pub fn name(&self) -> String {
        unsafe {
            CStr::from_ptr(spdk_bdev_get_name(self.0))
                .to_str()
                .unwrap()
                .to_string()
        }
    }

    /// the UUID that is set for this bdev, all bdevs should have a UUID set
    pub fn uuid(&self) -> Uuid {
        Uuid {
            0: unsafe { spdk_bdev_get_uuid(self.0) },
        }
    }

    /// converts the UUID to a string
    pub fn uuid_as_string(&self) -> String {
        let u = Uuid(unsafe { spdk_bdev_get_uuid(self.0) });
        let uuid = uuid::Uuid::from_bytes(u.as_bytes());
        uuid.to_hyphenated().to_string()
    }

    /// Set an alias on the bdev, this alias can be used to find the bdev later
    pub fn add_alias(&self, alias: &str) -> bool {
        let alias = std::ffi::CString::new(alias).unwrap();
        let ret =
            unsafe { spdk_sys::spdk_bdev_alias_add(self.0, alias.as_ptr()) };

        ret == 0
    }

    /// Get list of bdev aliases
    pub fn aliases(&self) -> Vec<String> {
        let mut aliases = Vec::new();
        let head = unsafe { &*spdk_bdev_get_aliases(self.0) };
        let mut ent_ptr = head.tqh_first;
        while !ent_ptr.is_null() {
            let ent = unsafe { &*ent_ptr };
            let alias = unsafe { CStr::from_ptr(ent.alias) };
            aliases.push(alias.to_str().unwrap().to_string());
            ent_ptr = ent.tailq.tqe_next;
        }
        aliases
    }

    /// returns whenever the bdev supports the requested IO type
    pub fn io_type_supported(&self, io_type: u32) -> bool {
        unsafe { spdk_bdev_io_type_supported(self.0, io_type) }
    }

    /// returns the bdev als a ptr
    pub fn as_ptr(&self) -> *mut spdk_bdev {
        self.0
    }

    /// convert a given UUID into a spdk_bdev_uuid or otherwise, auto generate
    /// one when uuid is None
    pub fn set_uuid(&mut self, uuid: Option<String>) {
        if let Some(uuid) = uuid {
            if let Ok(this_uuid) = uuid::Uuid::parse_str(&uuid) {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        this_uuid.as_bytes().as_ptr() as *const _
                            as *mut c_void,
                        &mut (*self.0).uuid.u.raw[0] as *const _ as *mut c_void,
                        (*self.0).uuid.u.raw.len(),
                    );
                }
                return;
            }
        }
        unsafe { spdk_uuid_generate(&mut (*self.0).uuid) };
        info!("No or invalid v4 UUID specified, using self generated one");
    }
}
