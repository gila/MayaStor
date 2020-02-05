use spdk_sys::{spdk_thread, spdk_set_thread, spdk_thread_poll};
use snafu::{Snafu};

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Event spawned from a non-spdk thread"))]
    InvalidThread {},
}

#[derive(Debug, Copy, Clone)]
/// struct that wraps an SPDK thread. The name thread is chosen poorly and
/// should not be confused with an actual thread. Consider it more to be
/// analogous to a container to which you can submit work and poll it to drive
/// the submitted work to completion.
pub struct Mthread(pub (crate) *mut spdk_thread);

unsafe impl Send for Mthread {}
unsafe impl Sync for Mthread {}

impl Mthread {
    ///
    /// With the given thread as context, execute the closure on that thread.
    ///
    /// Any function can be executed here however, this should typically be used
    /// to execute functions that reference any FFI to SPDK.
    ///
    /// # Note
    ///
    /// Avoid any blocking calls as it will block the reactor, and avoid
    /// long-running functions in general follow the nodejs event loop
    /// model, and you should be good.
    pub fn with<F: FnOnce()>(self, f: F) -> Self {

        unsafe { spdk_set_thread(self.0) };
        f();
        self.poll();
        unsafe { spdk_set_thread(std::ptr::null_mut()) };
        self
    }

    pub fn poll(&self) {
        let mut done = false;
        while !done {
            let rc = unsafe { spdk_thread_poll(self.0, 0, 0) };
            if rc < 1 {
                done = true
            }
        }
    }

    pub fn inner(self) -> *const spdk_thread {
        self.0
    }

    pub fn inner_mut(self) -> *mut spdk_thread {
        self.0
    }

    pub fn from_null_checked(t: *mut spdk_thread) -> Option<Self> {
        if t.is_null() {
            None
        } else {
            Some(Mthread(t))
        }
    }
}
