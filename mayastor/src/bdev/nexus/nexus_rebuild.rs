//!
//! This file contains the main routines that implement the rebuild process of
//! an nexus instance.

use crate::{
    bdev::nexus::{
        nexus_bdev::{Nexus, NexusState},
        nexus_child::ChildState,
        Error,
    },
    descriptor::Descriptor,
    rebuild::RebuildTask,
};
use std::rc::Rc;

impl Nexus {
    /// find any child that requires a rebuild. Children in the faulted state
    /// are eligible for a rebuild closed children are not and must be
    /// opened first.
    fn find_rebuild_target(&mut self) -> Option<Rc<Descriptor>> {
        if self.state != NexusState::Degraded {
            trace!(
                "{}: does not require any rebuild operation as its state: {}",
                self.name,
                self.state.to_string()
            );
        }

        for child in &self.children {
            if child.state == ChildState::Faulted {
                trace!(
                    "{}: child {} selected as rebuild target",
                    self.name,
                    child.name
                );
                return Some(child.borrow_descriptor());
            }
        }
        None
    }

    /// find a child which can be used as a rebuild source
    fn find_rebuild_source(&mut self) -> Option<Rc<Descriptor>> {
        if self.children.len() == 1 {
            trace!("{}: not enough children to initiate rebuild", self.name);
            return None;
        }

        for child in &self.children {
            if child.state == ChildState::Open {
                trace!(
                    "{}: child {} selected as rebuild source",
                    self.name,
                    child.name
                );
                return Some(child.borrow_descriptor());
            }
        }
        None
    }

    pub fn init_rebuild(&mut self) -> Result<(), Error> {
        if let Some(target) = self.find_rebuild_target() {
            if let Some(source) = self.find_rebuild_source() {
                let mut task = RebuildTask::new(source, target)?;
                task.nexus = Some(self.name.clone());
                self.rebuild_handle = Some(task);
                return Ok(());
            }
        }

        Err(Error::Internal(
            "{}: cannot construct rebuild solution".into(),
        ))
    }

    pub fn start_rebuild(&mut self) -> Result<NexusState, Error> {
        if let Some(task) = self.rebuild_handle.take() {
            self.rebuild_handle = Some(RebuildTask::start_rebuild(task)?);
            Ok(self.set_state(NexusState::Remuling))
        } else {
            Err(Error::Internal("no rebuild task configured".into()))
        }
    }

    pub async fn rebuild_completion(&mut self) -> Result<bool, Error> {
        if let Some(task) = self.rebuild_handle.as_mut() {
            if let Ok(r) = task.completed().await {
                let _ = self.rebuild_handle.take();
                Ok(r)
            } else {
                Ok(false)
            }
        } else {
            Err(Error::Invalid("No rebuild task registered".into()))
        }
    }
}
