use std::{
    convert::TryFrom,
    fmt::{Debug, Formatter},
};

use async_trait::async_trait;
use nix::errno::Errno;
use snafu::Snafu;

use crate::{
    bdev::{device_lookup, Uri},
    core::{BlockDevice, BlockDeviceDescriptor, CoreError},
    nexus_uri::{bdev_create, bdev_destroy},
    subsys::child::Inventory,
};

#[derive(Debug, Copy, Clone, Ord, PartialOrd, Eq, PartialEq)]
pub enum State {
    /// the child is the opening state -- this state is only valid during
    /// initial registration. The block device itself assumes to exist, but the
    /// device has not been opened.
    Init,
    /// the child is open and takes part of the normal IO path
    Open,
    /// the child is marked to be closing
    Closing,
    /// the child is getting closed and does not take part of the IO path
    Closed,
    /// the child is faulted for `[Reason]' and does not part in the IO path
    Faulted(Reason),
    Destroying,
    /// the underlying device is destroyed as well
    Destroyed,
}

impl ToString for State {
    fn to_string(&self) -> String {
        match *self {
            State::Init => "Opening",
            State::Open => "Open",
            State::Closing => "Closing",
            State::Closed => "Closed",
            State::Destroyed => "Destroyed",
            State::Destroying => "Destroying",
            State::Faulted(_) => "Faulted",
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
    CloseError {
        state: State,
        name: String,
    },
    OpenError {
        state: State,
        name: String,
    },
}

/// a child is an abstraction over BlockDevice this can either be a bdev or a
/// nvme device. As long as the descriptor is not released, the underlying block
/// device can not be destroyed. This is enforced by the bdev manager.
///
/// # Safety
///
/// As long as the block device has not been destroyed, this structure remains
/// valid. The only way to destroy a block device is by calling ['bdev_create']
/// and ['bdev_destroy']. A block device can not be destroyed while its opened.
/// An open block device implies we have a descriptor
pub struct Child {
    /// name of the child device -- this MAY be different from the BlockDevice
    /// it represents
    name: String,
    /// uri of the device used
    uri: String,
    /// device URI used to create the block device
    /// the device refers to the underlying block device. The block device
    /// should not be removed during the lifetime of the child without
    /// ensuring proper notifications are upheld
    block_device: Option<Box<dyn BlockDevice>>,
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
    state: State,
}

unsafe impl Send for Child {}

impl Debug for Child {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Child")
            .field("name", &self.name)
            .field("uri", &self.uri)
            .field("state", &self.state)
            .finish()
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        self.close().unwrap();
    }
}

// we can not mock freestanding functions so use a trait with a default impl to
// call into the standard device_lookup methods.
// TODO: create BaseBdevOps trait for base bdevs
#[async_trait(?Send)]
trait ChildBaseBdevOps {
    /// lookup a base block device
    fn base_bdev_lookup(&self, name: &str) -> Option<Box<dyn BlockDevice>> {
        device_lookup(name)
    }

    async fn base_bdev_destroy(&self, name: &str) -> Result<(), CoreError> {
        bdev_destroy(name)
            .await
            .map_err(|_e| CoreError::BdevNotFound {
                name: name.to_string(),
            })
    }

    async fn base_bdev_create(&self, uri: &str) -> Result<Child, CoreError> {
        let bdev = bdev_create(uri.as_ref()).await.map_err(|_e| {
            CoreError::OpenBdev {
                source: Errno::UnknownErrno,
            }
        })?;
        Child::try_from(bdev)
    }
}

impl TryFrom<String> for Child {
    type Error = CoreError;

    fn try_from(uri: String) -> Result<Self, Self::Error> {
        let name = Uri::parse(&uri)
            .map_err(|source| CoreError::ParseError {
                source,
            })?
            .get_name();
        if let Some(block_device) = device_lookup(name.as_ref()) {
            Ok(Self {
                name,
                uri,
                block_device: Some(block_device),
                descriptor: None,
                state: State::Init,
            })
        } else {
            Err(CoreError::BdevNotFound {
                name: name.into(),
            })
        }
    }
}

impl ChildBaseBdevOps for Child {}
#[cfg_attr(test, automock)]
impl Child {
    /// create a new child device in order for this to succeed we must have a
    /// valid underlying block device. Note that we do not open the device
    fn new(name: String) -> Result<Self, CoreError> {
        if Inventory::get().lookup(&name).is_some() {
            error!(?name, "already exists in child list");
            Err(CoreError::Exists)
        } else {
            Ok(Self {
                name: name.clone(),
                uri: name,
                block_device: None,
                descriptor: None,
                state: State::Init,
            })
        }
    }

    /// Destroys the underlying base block device and consumes self
    pub async fn destroy(&mut self) -> Result<(), CoreError> {
        if self.state() != State::Closed {
            return Err(CoreError::NotSupported {
                source: Errno::ENODEV,
            });
        }
        self.set_state(State::Destroying);
        let _ = self.base_bdev_destroy(&self.name).await.map_err(|_e| {
            self.set_state(State::Faulted(Reason::Missing));
            CoreError::OpenBdev {
                source: Errno::UnknownErrno,
            }
        })?;

        self.set_state(State::Destroyed);

        Ok(())
    }

    /// set the state to new state. if the state has transitioned it will return
    /// Some(State)
    fn set_state(&mut self, new_state: State) -> Option<State> {
        let old = self.state;
        self.state = new_state;
        if old == self.state {
            None
        } else {
            Some(old)
        }
    }

    /// returns the current state
    pub fn state(&self) -> State {
        self.state
    }

    pub fn uri(&self) -> String {
        self.uri.clone()
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    /// Opens the new child device which is intended to reflect a normal open.
    /// To open a child we must have an underlying block device. These are,
    /// in the normal case provided by the kernel but here we must have our
    /// own. Once we have a block device we must open it. Opening is done by
    /// getting a descriptor. As long as we have a hold onto a descriptor
    /// the block devices can not be destroyed.
    ///
    /// If the device was previously faulted, we will retry to create the child.
    /// If we fail to do, so we mark the device as missing.
    pub fn open(&mut self) -> Result<State, CoreError> {
        // if the device is already open, assert that it is in the proper state
        if matches!(self.state(), State::Open) {
            assert_eq!(self.descriptor.is_none(), false);
            assert_eq!(self.block_device.is_none(), false);
            assert_eq!(
                self.name,
                self.block_device.as_ref().unwrap().device_name()
            );
            return Ok(State::Open);
        }

        match self.state() {
            // we can open a device that is in the these states
            State::Init | State::Closed | State::Faulted(_) => {
                return if let Some(device) = self.base_bdev_lookup(&self.name) {
                    let desc = device.open(true)?;
                    self.descriptor = Some(desc);
                    self.block_device = Some(device);
                    self.set_state(State::Open);
                    Ok(self.state)
                } else {
                    self.set_state(State::Faulted(Reason::Missing));
                    Err(CoreError::BdevNotFound {
                        name: self.name.clone(),
                    })
                };
                // a device that is destroyed or in the process of being
                // destroyed, can not be opened and must be
                // recreated we do not know if a bdev_destroy()
                // has been issued or if an async callback is
                // pending. Even if we did, we can not cancel
                // this request.
            }
            State::Destroyed | State::Destroying => {
                return Err(CoreError::BdevNotFound {
                    name: self.name.clone(),
                });
            }
            _ => {
                panic!("dont know how to handle this case")
            }
        }
    }

    /// Close the child, will close the associated descriptor. It
    /// will not however, destroy the underlying device. We only drop the
    /// descriptor and the device such that eventually, the device MAY be
    /// destroyed.
    pub fn close(&mut self) -> Result<State, Error> {
        return if matches!(self.state(), State::Open) {
            self.descriptor.take();
            self.block_device.take();
            // if the child was open, set it to the init state
            self.set_state(State::Closed);
            Ok(self.state())
        } else {
            Err(Error::CloseError {
                state: self.state(),
                name: self.name.clone(),
            })
        };
    }
    /// fault a child for some ['Reason'] faulting a child does nothing other
    /// than updating its state
    pub fn fault(&mut self, r: Reason) -> Result<State, Error> {
        let current = self.state();

        if matches!(current, State::Faulted(_)) {
            info!(?self.name, ?current, "already faulted");
            return Ok(current);
        }
        self.set_state(State::Faulted(r));
        Ok(State::Faulted(r))
    }
}
