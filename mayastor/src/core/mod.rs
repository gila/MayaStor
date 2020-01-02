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
use snafu::Snafu;

#[derive(Debug, Snafu)]
pub enum CoreError {
    #[snafu(display("bdev {} not found", name))]
    BdevOpen { name: String },
    #[snafu(display("failed to get IO channel for {}", name))]
    GetIoChannel { name: String },
}
