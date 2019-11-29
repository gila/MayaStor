use crate::bdev::nexus::Error;
use spdk_sys::{spdk_event, spdk_event_allocate, spdk_event_call};
use std::{os::raw::c_void, pin::Pin, rc::Rc};

#[derive(Debug)]
pub struct Event<T> {
    /// pointer to the allocated event
    inner: Option<*mut spdk_event>,
    pub task: Pin<Box<T>>,
}

pub type EventFn = extern "C" fn(*mut c_void, *mut c_void);

impl<T> Event<T> {
    /// create a new event that can be called later by the reactor. T will be
    /// forgotten and passed over to FFI. If this function returns an error,
    /// T is implicitly dropped as it consumes T when called.
    pub(crate) fn new(
        core: u32,
        start_fn: EventFn,
        argx: Box<T>,
    ) -> Result<Box<T>, Error> {
        let ptr = &*argx as *const _ as *mut c_void;
        let inner = unsafe {
            spdk_event_allocate(core, Some(start_fn), ptr, std::ptr::null_mut())
        };

        if inner.is_null() {
            // take a hold of the data again to ensure it is dropped
            Err(Error::Internal("failed to allocate event".into()))
        } else {
            unsafe { spdk_event_call(inner) };

            Ok(argx)
        }
    }

    /// call the event (or more accurately add it to the reactor) when called
    /// the event is put back into the pool
    pub fn call(&mut self) {
        if let Some(inner) = self.inner {
            unsafe { spdk_event_call(inner) };
        }
    }
}
