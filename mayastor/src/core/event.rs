use snafu::Snafu;
use spdk_sys::{
    spdk_event_allocate,
    spdk_event_call,
    spdk_get_thread,
    spdk_set_thread,
    spdk_thread,
    spdk_thread_create,
    spdk_thread_poll,
};
use std::os::raw::c_void;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Event spawned from a non-spdk thread"))]
    InvalidThread {},
}

