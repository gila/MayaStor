use std::{
    collections::HashMap,
    convert::TryFrom,
    fmt::{Debug, Formatter},
    sync::{Arc, Mutex, RwLock},
};

use once_cell::sync::Lazy;
use snafu::Snafu;

use crate::{
    bdev::device_lookup,
    core::{BlockDevice, BlockDeviceDescriptor, CoreError},
};

#[derive(Debug, Ord, PartialOrd, Eq, PartialEq)]
pub enum State {
    Opening,
    Open,
    Closing,
    Closed,
    Faulted(Reason),
}

#[derive(Debug, Ord, PartialOrd, Eq, PartialEq)]
pub enum Reason {
    IOErrors,
    Missing,
}

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to open child"))]
    OpenChild {
        source: CoreError,
    },
    Exists,
}

#[derive(Debug, Default)]
pub struct ChildList {
    entries: RwLock<HashMap<String, Arc<Mutex<Child>>>>,
}

impl ChildList {
    /// insert a child in the global child list returning a child in the open
    /// state
    pub fn insert(&self, name: String, c: Arc<Mutex<Child>>) {
        info!(?name, "inserting into child list");
        let old = self
            .entries
            .write()
            .expect("Child list poisoned")
            .insert(name, Arc::clone(&c));

        assert_eq!(c.lock().unwrap().state, State::Opening);
        c.lock().unwrap().state = State::Open;

        if old.is_some() {
            warn!("duplicate entry existed... this this entry will be dropped now!");
        }
    }

    pub fn lookup<N: Into<String>>(
        &self,
        name: N,
    ) -> Option<Arc<Mutex<Child>>> {
        self.entries
            .read()
            .expect("child list poisoned")
            .get(&name.into())
            .map(|c| Arc::clone(&c))
    }

    pub fn drop_all(&self) {
        self.entries.write().unwrap().clear();
    }
}

/// global list of child devices
pub static CHILD_LIST: Lazy<ChildList> = Lazy::new(ChildList::default);
/// a child is an abstraction over BlockDevice this can either be a bdev or a
/// nvme device. As long as the descriptor is not released, the underlying block
/// device can not be destroyed. This is enforced by the bdev manager.
pub struct Child {
    /// name of the child device -- this MAY be different from the BlockDevice
    /// it represents
    name: String,
    /// the device refers to the underlying block device. The block device
    /// should not be removed during the lifetime of the child without
    /// ensuring proper notifications are upheld
    device: Option<Box<dyn BlockDevice>>,
    /// descriptor to the device which allows getting basic information of the
    /// device itself the descriptor can not be directly used to perform IO
    /// operations. To perform IO a handle for this thread must be obtained
    descriptor: Option<Box<dyn BlockDeviceDescriptor>>,
    /// the interior state of the child. The child state is needed to ensure
    /// proper synchronisation with the raw FFI layer. It may occur that raw
    /// RPC calls are made that remove the block device. This is discouraged
    /// but can still happen, and so we must account for it
    state: State,
}

unsafe impl Send for Child {}

impl Debug for Child {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Child")
            .field("name", &self.name)
            .field("state", &self.state)
            .field("product", &self.device.as_ref().unwrap().product_name())
            .field("claimed_by", &self.device.as_ref().unwrap().claimed_by())
            .finish()
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        self.descriptor.take().unwrap();
        self.device.take().unwrap();
    }
}

impl TryFrom<String> for Child {
    type Error = Error;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        if let Some(device) = device_lookup(name.as_ref()) {
            let descriptor =
                device.open(true).map_err(|source| Error::OpenChild {
                    source,
                })?;

            Ok(Self {
                name: device.device_name(),
                device: Some(device),
                descriptor: Some(descriptor),
                state: State::Opening,
            })
        } else {
            Err(Error::OpenChild {
                source: CoreError::BdevNotFound {
                    name: name.into(),
                },
            })
        }
    }
}

impl Child {
    pub fn new(name: String) -> Result<Arc<Mutex<Self>>, Error> {
        if CHILD_LIST.lookup(&name).is_some() {
            error!(?name, "already exists in child list");
            return Err(Error::Exists);
        } else {
            let child = Arc::new(Mutex::new(Child::try_from(name.clone())?));
            CHILD_LIST.insert(name, Arc::clone(&child));
            Ok(child)
        }
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }
}
