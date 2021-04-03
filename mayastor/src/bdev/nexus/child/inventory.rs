use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
};

use nix::errno::Errno;
use once_cell::sync::Lazy;

use crate::{
    bdev::nexus::child::Child,
    core::CoreError,
    nexus_uri::bdev_create,
};
use std::convert::TryFrom;

/// Global list of child devices, when the list is consulted a ChildEntry will
/// be returned

pub static CHILD_INVENTORY: Lazy<Inventory> = Lazy::new(Inventory::default);

pub type ChildEntry = Arc<Mutex<Child>>;
#[derive(Debug, Default)]
pub struct Inventory {
    entries: RwLock<HashMap<String, ChildEntry>>,
}

impl Inventory {
    /// insert a child in the global child list returning the child as an
    /// Arc<Mutex> state
    fn insert(&self, c: Child) -> Result<ChildEntry, CoreError> {
        info!(?c, "inserting into child list");
        let name = c.name.clone();

        let mut entries =
            self.entries.write().expect("child list lock poisoned");

        if entries.contains_key(&name) {
            return Err(CoreError::Exists);
        }

        let child = Arc::new(Mutex::new(c));

        if let Some(old) = entries.insert(name, Arc::clone(&child)) {
            error!(?old, "child list corrupted -- dropping entry that should not be there");
        }

        Ok(child)
    }

    /// This is a freestanding function and there is no mutex held as the
    /// child does not exist at this point. The block device shall be created
    /// and inserted into the list. From here the child can be opened.
    pub async fn create<N: AsRef<str>>(
        &self,
        uri: N,
    ) -> Result<ChildEntry, CoreError> {
        let bdev = bdev_create(uri.as_ref()).await.map_err(|_e| {
            CoreError::OpenBdev {
                source: Errno::UnknownErrno,
            }
        })?;
        let child = Child::try_from(bdev)?;
        self.insert(child)
    }

    pub fn lookup<N: Into<String>>(&self, name: N) -> Option<ChildEntry> {
        self.entries
            .read()
            .expect("child list poisoned")
            .get(&name.into())
            .map(|c| Arc::clone(&c))
    }
    /// take the specified child out of the list
    pub fn take<N: Into<String>>(&self, name: N) -> Option<ChildEntry> {
        self.entries
            .write()
            .expect("child list poisoned")
            .remove(&name.into())
    }

    pub fn drop_all(&self) {
        self.entries.write().unwrap().clear();
    }
}
