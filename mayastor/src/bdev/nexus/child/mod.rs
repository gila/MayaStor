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
use crossbeam::atomic::AtomicCell;

#[derive(Debug, Copy, Clone, Ord, PartialOrd, Eq, PartialEq)]
pub enum State {
    /// the child is the opening state -- this state is only valid during
    /// initial registration
    Init,
    /// the child is open and takes part of the normal IO path
    Open,
    /// the child is marked to be closing
    Closing,
    /// the child is getting closed and does not take part of the IO path
    Closed,
    /// the child is faulted for `[Reason]' and does not part in the IO path
    Faulted(Reason),
}

impl ToString for State {
    fn to_string(&self) -> String {
        match *self {
            State::Init => "Opening",
            State::Open => "Open",
            State::Closing => "Closing",
            State::Closed => "Closed",
            State::Faulted(r) => "Faulted",
        }
        .to_string()
    }
}

#[derive(Debug, Copy, Clone, Ord, PartialOrd, Eq, PartialEq)]
pub enum Reason {
    IOErrors,
    Missing,
}

impl ToString for Reason {
    fn to_string(&self) -> String {
        match *self {
            Reason::IOErrors => "Faulted(to many IO errors)",
            Reason::Missing => "Faulted(missing)",
        }
        .to_string()
    }
}

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to open child"))]
    OpenChild {
        source: CoreError,
    },
    Exists,
    Unknown,
    OpenError {
        state: State,
        name: String,
    },
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

        c.lock()
            .unwrap()
            .open()
            .map(|s| assert_eq!(s, State::Open))
            .unwrap();

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
    /// but can still happen, and so we must account for it. The state does
    /// not need to be atomic as the Child is guarded by a Mutex. However, we
    /// want to prepare for lifting the mutex as it really should not be
    /// needed to have the child itself, be guarded by a mutex.
    state: AtomicCell<State>,
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
        self.close();
    }
}

impl TryFrom<String> for Child {
    type Error = Error;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        if let Some(device) = device_lookup(name.as_ref()) {
            Ok(Self {
                name: device.device_name(),
                device: Some(device),
                descriptor: None,
                state: AtomicCell::new(State::Init),
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
    /// create a new child device in order for this to succeed we must have a
    /// valid underlying block device
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

    /// set the state to new state. if the state has transitioned it will return
    /// Some(State)
    fn set_state(&self, new_state: State) -> Option<State> {
        let old = self.state.swap(new_state);
        if old == new_state {
            None
        } else {
            Some(old)
        }
    }

    /// returns the current state
    pub fn state(&self) -> State {
        self.state.load()
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    /// Opens the new child device which is intended to reflect a normal open.
    /// To open a child we must have an underlying block device. These are,
    /// in the normal case provided by the kernel but here we must have our
    /// own. Once we have a block device we must open it. Opening is done by
    /// getting a descriptor. As long as we have a hold onto a descriptor
    /// the block devices can not be destroyed
    pub fn open(&mut self) -> Result<State, Error> {
        let current = self.state();
        if matches!(current, State::Open) {
            assert_eq!(self.descriptor.is_none(), false);
            assert_eq!(self.device.is_none(), false);
            return Ok(State::Open);
        }

        return if matches!(
            current,
            State::Init | State::Closed | State::Faulted(_)
        ) {
            if let Some(device) = device_lookup(&self.name) {
                let desc = device.open(true)?;
                self.descriptor = Some(desc);
                self.device = Some(device);
                self.set_state(State::Open);
                Ok(self.state())
            } else {
                self.set_state(State::Faulted(Reason::Missing));
                Err(Error::OpenError {
                    state: self.state(),
                    name: self.name.clone(),
                })
            }
        } else {
            panic!("unhandled child state...");
        };
    }

    /// Close the child, will close the associated channel and descriptor. It
    /// will not however, destroy the underlying device. We only drop the
    /// descriptor and the device such that eventually, the device MAY be
    /// destroyed.
    pub fn close(&mut self) -> Result<State, Error> {
        let current = self.state();
        return if matches!(current, State::Faulted(_) | State::Open) {
            self.descriptor
                .take()
                .expect("trying to close a child with no descriptor");
            self.device.take().expect("device is gone");
            self.set_state(State::Init);
            Ok(State::Init)
        } else {
            Err(Error::OpenError {
                state: self.state(),
                name: self.name.clone(),
            })
        };
    }
}
