use std::{convert::TryFrom, mem::ManuallyDrop, sync::Arc};

use crate::{
    core::{Bdev, CoreError, Descriptor, IoChannel},
    dma::{DmaBuf, DmaError},
    executor::cb_arg,
};
use futures::channel::oneshot;
use spdk_sys::{
    spdk_bdev_desc,
    spdk_bdev_free_io,
    spdk_bdev_io,
    spdk_bdev_read,
    spdk_bdev_write,
    spdk_io_channel,
};
use std::os::raw::c_void;

use nix::errno::Errno;
use serde::export::{fmt::Error, Formatter};
use std::fmt::Debug;

/// A handle to a bdev, is an interface to submit IO.
pub struct BdevHandle {
    pub desc: ManuallyDrop<Arc<Descriptor>>,
    pub channel: ManuallyDrop<IoChannel>,
}

impl BdevHandle {
    pub fn open(
        name: &str,
        read_write: bool,
        claim: bool,
    ) -> Result<BdevHandle, CoreError> {
        if let Ok(desc) = Bdev::open_by_name(name, read_write) {
            if claim && !desc.claim() {
                return Err(CoreError::BdevOpen {
                    name: name.into(),
                });
            }
            return BdevHandle::try_from(Arc::new(desc));
        }

        Err(CoreError::BdevOpen {
            name: name.into(),
        })
    }

    pub fn close(self) {
        drop(self);
    }

    pub fn get_bdev(&self) -> Bdev {
        self.desc.get_bdev()
    }

    pub fn io_tuple(&self) -> (*mut spdk_bdev_desc, *mut spdk_io_channel) {
        (self.desc.as_ptr(), self.channel.as_ptr())
    }

    /// Allocate memory from the memory pool (the mem is zeroed out)
    /// with given size and proper alignment for the bdev.
    pub fn dma_malloc(&self, size: usize) -> Result<DmaBuf, DmaError> {
        DmaBuf::new(size, self.desc.get_bdev().alignment())
    }

    /// io completion callback that sends back the success status of the IO.
    /// When the IO is freed, it is returned to the memory pool. The buffer is
    /// not freed this is not very optimal right now, as we use oneshot
    /// channels from futures 0.3 which (AFAIK) does not have unsync support
    /// yet.
    extern "C" fn io_completion_cb(
        io: *mut spdk_bdev_io,
        success: bool,
        arg: *mut c_void,
    ) {
        let sender = unsafe {
            Box::from_raw(arg as *const _ as *mut oneshot::Sender<bool>)
        };

        unsafe {
            spdk_bdev_free_io(io);
        }

        sender.send(success).expect("io completion error");
    }

    /// write the `buffer` to the given `offset`
    pub async fn write_at(
        &self,
        offset: u64,
        buffer: &DmaBuf,
    ) -> Result<usize, CoreError> {
        if offset % u64::from(self.desc.get_bdev().block_len()) != 0 {
            return Err(CoreError::InvalidOffset {
                offset,
            });
        }

        let (s, r) = oneshot::channel::<bool>();
        let errno = unsafe {
            spdk_bdev_write(
                self.desc.as_ptr(),
                self.channel.as_ptr(),
                **buffer,
                offset,
                buffer.len() as u64,
                Some(Self::io_completion_cb),
                cb_arg(s),
            )
        };

        if errno != 0 {
            return Err(CoreError::WriteDispatch {
                source: Errno::from_i32(errno),
                offset,
                len: buffer.len(),
            });
        }

        if r.await.expect("Failed awaiting write IO") {
            Ok(buffer.len() as usize)
        } else {
            Err(CoreError::WriteFailed {
                offset,
                len: buffer.len(),
            })
        }
    }

    /// read from the given `offset` into the `buffer` note that the buffer
    /// is allocated internally and should be copied. Also, its unknown to me
    /// what will happen if you for example, where to turn this into a vec
    /// but for sure -- not what you want.
    pub async fn read_at(
        &self,
        offset: u64,
        buffer: &mut DmaBuf,
    ) -> Result<usize, CoreError> {
        if offset % u64::from(self.desc.get_bdev().block_len()) != 0 {
            return Err(CoreError::InvalidOffset {
                offset,
            });
        }
        let (s, r) = oneshot::channel::<bool>();
        let errno = unsafe {
            spdk_bdev_read(
                self.desc.as_ptr(),
                self.channel.as_ptr(),
                **buffer,
                offset,
                buffer.len() as u64,
                Some(Self::io_completion_cb),
                cb_arg(s),
            )
        };

        if errno != 0 {
            return Err(CoreError::ReadDispatch {
                source: Errno::from_i32(errno),
                offset,
                len: buffer.len(),
            });
        }

        if r.await.expect("Failed awaiting read IO") {
            Ok(buffer.len())
        } else {
            Err(CoreError::ReadFailed {
                offset,
                len: buffer.len(),
            })
        }
    }
}

impl Drop for BdevHandle {
    fn drop(&mut self) {
        unsafe {
            trace!("{:?}", self);
            //            self.desc.release();
            // the order of dropping has to be deterministic
            ManuallyDrop::drop(&mut self.channel);
            ManuallyDrop::drop(&mut self.desc);
        }
    }
}

impl Debug for BdevHandle {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "{:?}", self.desc)?;
        write!(f, "{:?}", self.channel)
    }
}

impl TryFrom<Descriptor> for BdevHandle {
    type Error = CoreError;

    fn try_from(desc: Descriptor) -> Result<Self, Self::Error> {
        if let Some(channel) = desc.get_channel() {
            return Ok(Self {
                desc: ManuallyDrop::new(Arc::new(desc)),
                channel: ManuallyDrop::new(channel),
            });
        }

        Err(CoreError::GetIoChannel {
            name: desc.get_bdev().name(),
        })
    }
}

impl TryFrom<Arc<Descriptor>> for BdevHandle {
    type Error = CoreError;

    fn try_from(desc: Arc<Descriptor>) -> Result<Self, Self::Error> {
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
