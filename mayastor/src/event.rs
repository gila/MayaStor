use crate::{bdev::nexus::Error, rebuild::MayaCtx};
use spdk_sys::{spdk_event_allocate, spdk_event_call};
use std::os::raw::c_void;

pub type EventFn = extern "C" fn(*mut c_void, *mut c_void);

/// dispatch an event to the reactor, if we fail to allocate an event we return
/// error otherwise we return Ok with the argument originally passed in.
pub(crate) fn dispatch<T>(
    core: u32,
    start_fn: EventFn,
    argx: Box<T>,
) -> Result<Box<T>, Error> {
    let ptr = &*argx as *const _ as *mut c_void;
    let inner = unsafe {
        spdk_event_allocate(core, Some(start_fn), ptr, std::ptr::null_mut())
    };

    if inner.is_null() {
        return Err(Error::Internal("failed to allocate event".into()));
    } else {
        unsafe { spdk_event_call(inner) };
    }

    Ok(argx)
}

pub fn run_on_core<T: MayaCtx, F: FnOnce(&mut T::Item)>(
    core: u32,
    arg: Box<T>,
    f: F,
) -> Result<Box<T>, Error> {
    extern "C" fn unwrap<F, T>(f: *mut c_void, t: *mut c_void)
    where
        F: FnOnce(&mut T::Item),
        T: MayaCtx,
    {
        unsafe {
            let f: Box<F> = Box::from_raw(f as *mut F);
            let arg = T::into_ctx(t);
            f(arg)
        }
    }

    let ptr = Box::into_raw(Box::new(f)) as *mut c_void;
    let arg_ptr = &*arg as *const _ as *mut c_void;
    let event = unsafe {
        spdk_event_allocate(core, Some(unwrap::<F, T>), ptr, arg_ptr)
    };

    if event.is_null() {
        panic!("failed to allocate event");
    }
    unsafe { spdk_event_call(event) };
    Ok(arg)
}

pub fn on_core<F: FnOnce()>(core: u32, f: F) {
    extern "C" fn unwrap<F>(args: *mut c_void, arg2: *mut c_void)
    where
        F: FnOnce(),
    {
        unsafe {
            let f: Box<F> = Box::from_raw(args as *mut F);
            f()
        }
    }
    let ptr = Box::into_raw(Box::new(f)) as *mut c_void;
    let event = unsafe {
        spdk_event_allocate(core, Some(unwrap::<F>), ptr, std::ptr::null_mut())
    };

    if event.is_null() {
        panic!("failed to allocate event");
    }
    unsafe { spdk_event_call(event) }
}
