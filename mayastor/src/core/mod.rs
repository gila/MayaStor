//!
//! core contains the primary abstractions around the SPDK primitives.

mod bdev;
mod channel;
mod descriptor;
mod handle;
mod uuid;

pub use ::uuid::Uuid;
pub use bdev::Bdev;
pub use channel::IoChannel;
pub use descriptor::Descriptor;
pub use handle::BdevHandle;
use nix::errno::Errno;
use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(visibility = "pub")]
pub enum CoreError {
    #[snafu(display("bdev {} not found", name))]
    BdevOpen {
        name: String,
    },
    #[snafu(display("bdev {} not found", name))]
    InvalidDescriptor {
        name: String,
    },
    #[snafu(display("failed to get IO channel for {}", name))]
    GetIoChannel {
        name: String,
    },
    InvalidOffset {
        offset: u64,
    },
    #[snafu(display(
        "Failed to dispatch write at offset {} length {}",
        offset,
        len
    ))]
    WriteDispatch {
        source: Errno,
        offset: u64,
        len: usize,
    },
    #[snafu(display(
        "Failed to dispatch read at offset {} length {}",
        offset,
        len
    ))]
    ReadDispatch {
        source: Errno,
        offset: u64,
        len: usize,
    },
    #[snafu(display("Write failed at offset {} length {}", offset, len))]
    WriteFailed {
        offset: u64,
        len: usize,
    },
    #[snafu(display("Read failed at offset {} length {}", offset, len))]
    ReadFailed {
        offset: u64,
        len: usize,
    },
}
