use std::{
    ffi::{c_void, CString},
    ptr::NonNull,
    time::Duration,
};

use spdk_sys::{
    spdk_poller,
    spdk_poller_pause,
    spdk_poller_register,
    spdk_poller_register_named,
    spdk_poller_resume,
    spdk_poller_unregister,
};
use std::fmt::Debug;

/// structure holding our function and context
struct PollCtx<'a, T: Send + std::fmt::Debug> {
    poll_fn: Box<dyn FnMut(&T) -> i32 + 'a>,
    poll_args: T,
}

/// indirection to avoid raw pointers at upper layers
#[inline(always)]
extern "C" fn _cb<T: Send + Debug>(ctx: *mut c_void) -> i32 {
    let poll = unsafe { &mut *(ctx as *mut PollCtx<T>) };
    (poll.poll_fn)(&poll.poll_args)
}

/// Poller structure that allows us to pause, stop, resume periodic tasks
pub struct Poller<'a, T: Send + Debug> {
    inner: NonNull<spdk_poller>,
    ctx: NonNull<PollCtx<'a, T>>,
    stopped: bool,
}

impl<'a, T: Send + Debug> Poller<'a, T> {
    /// stop the given poller and take ownership of the passed in closure or
    /// function
    pub fn stop(mut self) {
        unsafe {
            spdk_poller_unregister(&mut self.inner.as_ptr());
            Box::from_raw(self.ctx.as_ptr());
            self.stopped = true;
        }
    }

    /// pause the given poller
    pub fn pause(&mut self) {
        unsafe {
            spdk_poller_pause(self.inner.as_ptr());
        }
    }

    /// resume the given poller
    pub fn resume(&mut self) {
        unsafe {
            spdk_poller_resume(self.inner.as_ptr());
        }
    }
}

impl<'a, T: Send + Debug> Drop for Poller<'a, T> {
    fn drop(&mut self) {
        if !self.stopped {
            unsafe {
                spdk_poller_unregister(&mut self.inner.as_ptr());
                Box::from_raw(self.ctx.as_ptr());
            }
        }
    }
}

/// builder type to create a new poller
pub struct Builder<'a, T> {
    name: Option<CString>,
    interval: std::time::Duration,
    poll_fn: Option<Box<dyn FnMut(&T) -> i32 + 'a>>,
    ctx: Option<T>,
}

impl<'a, T: Send + Debug> Default for Builder<'a, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a, T: Send + Debug> Builder<'a, T> {
    /// create a new nameless poller that runs every time the thread the poller
    /// is created on is polled
    pub fn new() -> Self {
        Self {
            name: None,
            interval: Duration::from_micros(0),
            poll_fn: None,
            ctx: None,
        }
    }

    /// create the poller with a given name
    pub fn with_name<S: Into<Vec<u8>>>(mut self, name: S) -> Self {
        self.name = Some(
            CString::new(name)
                .expect("poller name is invalid or out of memory"),
        );
        self
    }

    /// set the interval for the poller in usec
    pub fn with_interval(mut self, usec: u64) -> Self {
        self.interval = Duration::from_micros(usec);
        self
    }

    /// set the function for this poller
    pub fn with_poll_fn(mut self, poll_fn: impl FnMut(&T) -> i32 + 'a) -> Self {
        self.poll_fn = Some(Box::new(poll_fn));
        self
    }

    pub fn with_ctx(mut self, ctx: T) -> Self {
        self.ctx = Some(ctx);
        self
    }

    /// build a  new poller object
    #[must_use]
    pub fn build(mut self) -> Poller<'a, T> {
        let poll_fn = self
            .poll_fn
            .take()
            .expect("can not start poller without poll function");

        let poll_args = self.ctx.take().expect("a context for the poller must be provided, consider using std::ptr::null otherwise");

        let ctx = NonNull::new(Box::into_raw(Box::new(PollCtx {
            poll_fn,
            poll_args,
        })))
        .expect("failed to allocate new poller context");

        let inner = NonNull::new(unsafe {
            if self.name.is_none() {
                spdk_poller_register(
                    Some(_cb::<T>),
                    ctx.as_ptr().cast(),
                    self.interval.as_micros() as u64,
                )
            } else {
                spdk_poller_register_named(
                    Some(_cb::<T>),
                    ctx.as_ptr().cast(),
                    self.interval.as_micros() as u64,
                    self.name.as_ref().unwrap().as_ptr(),
                )
            }
        })
        .expect("failed to register poller");

        Poller {
            inner,
            ctx,
            stopped: false,
        }
    }
}
