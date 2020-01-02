use std::{convert::TryFrom, mem::ManuallyDrop};

use crate::core::{Bdev, CoreError, Descriptor, IoChannel};

/// A handle to a bdev, is an interface to submit IO.
#[derive(Debug, Clone)]
pub struct BdevHandle {
    pub desc: ManuallyDrop<Descriptor>,
    pub channel: ManuallyDrop<IoChannel>,
}

impl Drop for BdevHandle {
    fn drop(&mut self) {
        unsafe {
            debug!("dropping handle");
            self.desc.release();
            // the order of dropping has to be deterministic
            ManuallyDrop::drop(&mut self.channel);
            ManuallyDrop::drop(&mut self.desc);
        }
    }
}

impl TryFrom<Descriptor> for BdevHandle {
    type Error = CoreError;

    fn try_from(desc: Descriptor) -> Result<Self, Self::Error> {
        if let Some(channel) = desc.get_channel() {
            return Ok(Self {
                desc: ManuallyDrop::new(desc),
                channel: ManuallyDrop::new(channel),
            });
        }

        Err(CoreError::GetIoChannel {
            name: desc.get_bdev().name(),
        })
    }
}

impl BdevHandle {
    pub fn open(
        name: &str,
        read_write: bool,
        claim: bool,
    ) -> Result<BdevHandle, CoreError> {
        if let Some(desc) = Bdev::open(name, read_write) {
            if claim && !desc.claim() {
                return Err(CoreError::BdevOpen {
                    name: name.into(),
                });
            }
            return BdevHandle::try_from(desc);
        }

        Err(CoreError::BdevOpen {
            name: name.into(),
        })
    }

    pub fn close(self) {
        drop(self);
    }
}
