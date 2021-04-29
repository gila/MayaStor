//!
//! Main file to register additional subsystems

pub use config::{
    opts::{NexusOpts, NvmeBdevOpts},
    BaseBdev,
    Config,
    ConfigSubsystem,
    NexusBdev,
    Pool,
};
use futures::{
    channel::oneshot::Receiver,
    task::{Context, Poll},
    Future,
};
pub use nvmf::{
    create_snapshot,
    set_snapshot_time,
    Error as NvmfError,
    NvmeCpl,
    NvmfReq,
    NvmfSubsystem,
    SubType,
    Target as NvmfTarget,
};
use spdk_sys::{
    spdk_add_subsystem,
    spdk_add_subsystem_depend,
    spdk_subsystem_depend,
};
use std::{
    fmt::{Debug, Display},
    mem::ManuallyDrop,
    pin::Pin,
};

pub use mbus::{
    mbus_endpoint,
    message_bus_init,
    registration::Registration,
    MessageBusSubsystem,
};

use crate::{
    core::{CoreError, Cores, Mthread},
    subsys::nvmf::Nvmf,
};

mod config;
mod mbus;
mod nvmf;

#[derive(Debug)]
pub struct Service {
    name: String,
    core: Cores,
    thread: Mthread,
}

impl Drop for Service {
    fn drop(&mut self) {
        info!(?self, "dropping service");
        self.thread.destroy();
    }
}

impl Service {
    pub fn new(name: String, core: Cores) -> Self {
        let thread = Mthread::new(name.clone(), core.id())
            .expect("failed to create service");
        Self {
            name,
            core,
            thread,
        }
    }

    pub fn with(&self, f: impl FnOnce()) {
        // wrap the closure into a checker that validates runtime constraints
        let thread = self.thread;

        self.thread.on(move || {
            assert_eq!(
                thread.into_raw(),
                Mthread::current().unwrap().into_raw()
            );
            f()
        });
    }

    pub fn with_cb<F, C:, R >(&self, f: F, callback: C)
    where
        F: FnOnce() -> R,
        C: FnMut(R),
        R: Send + Debug,
    {
        self.thread.on_cb(f, callback);
    }

    pub fn spawn_local<F: 'static, R: 'static, E: 'static>(
        &self,
        f: F,
    ) -> Result<Receiver<Result<R, E>>, CoreError>
    where
        F: Future<Output = Result<R, E>>,
        R: Send + Debug,
        E: Send + Display + Debug,
    {
        struct Checked<F> {
            thread: Mthread,
            inner: ManuallyDrop<F>,
        }

        impl<F> Drop for Checked<F> {
            fn drop(&mut self) {
                assert_eq!(Mthread::current().unwrap(), self.thread);
                info!("running on {:?}", Mthread::current());
                unsafe {
                    ManuallyDrop::drop(&mut self.inner);
                }

                self.thread.exit();
            }
        }

        impl<F: Future> Future for Checked<F> {
            type Output = F::Output;

            fn poll(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<Self::Output> {
                self.thread.enter();

                info!("running on {:?}", Mthread::current());
                unsafe { self.map_unchecked_mut(|c| &mut *c.inner).poll(cx) }
            }
        }

        // Wrap the future into one that checks which thread it's on.
        let future = Checked {
            thread: self.thread,
            inner: ManuallyDrop::new(f),
        };

        info!(?self.thread, "dispatching future on");
        self.thread.spawn_local(future)
    }
}

/// Register initial subsystems
pub(crate) fn register_subsystem() {
    unsafe { spdk_add_subsystem(ConfigSubsystem::new().0) }
    unsafe {
        let mut depend = Box::new(spdk_subsystem_depend::default());
        depend.name = b"mayastor_nvmf_tgt\0" as *const u8 as *mut _;
        depend.depends_on = b"bdev\0" as *const u8 as *mut _;
        spdk_add_subsystem(Nvmf::new().0);
        spdk_add_subsystem_depend(Box::into_raw(depend));
    }
    MessageBusSubsystem::register();
}
