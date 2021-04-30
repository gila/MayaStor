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

use crate::ffihelper::{
    cb_arg,
    done_errno_cb,
    AsStr,
    ErrnoResult,
    FfiResult,
    IntoCString,
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
    TargetState,
};

use crate::subsys::nvmf::NVMF_PGS;
use nvmf::transport::{get_ipv4_address, TransportId};
use spdk_sys::{
    spdk_add_subsystem,
    spdk_add_subsystem_depend,
    spdk_nvmf_listen_opts,
    spdk_nvmf_listen_opts_init,
    spdk_nvmf_target_opts,
    spdk_nvmf_tgt,
    spdk_nvmf_tgt_add_transport,
    spdk_nvmf_tgt_create,
    spdk_nvmf_tgt_listen_ext,
    spdk_nvmf_transport_create,
    spdk_subsystem_depend,
};

use std::{
    fmt::{Debug, Display},
    mem::ManuallyDrop,
    pin::Pin,
    ptr::NonNull,
};

pub use mbus::{
    mbus_endpoint,
    message_bus_init,
    registration::Registration,
    MessageBusSubsystem,
};

use crate::{
    core::{CoreError, Cores, Mthread, Reactors},
    subsys::nvmf::Nvmf,
};

mod config;
mod mbus;
pub mod nvmf;
use crate::subsys::nvmf::PollGroup;
impl Drop for Service {
    fn drop(&mut self) {
        info!(?self, "dropping service");
        self.thread.destroy();
    }
}

#[derive(Clone, Copy, Debug)]
struct NvmfTgt {
    tgt: NonNull<spdk_nvmf_tgt>,
    state: TargetState,
}

impl NvmfTgt {
    pub fn new() -> Self {
        let cfg = Config::get();
        let tgt_ptr: Box<spdk_nvmf_target_opts> =
            cfg.nvmf_tcp_tgt_conf.clone().into();

        let tgt =
            unsafe { spdk_nvmf_tgt_create(&*tgt_ptr as *const _ as *mut _) };
        Self {
            tgt: NonNull::new(tgt).unwrap(),
            state: TargetState::Init,
        }
    }

    pub async fn init_poll_groups(&self) {
        //NVMF_TARGET.get().unwrap().thread.enter();
        let mut waiter = Vec::new();
        Reactors::iter().for_each(|r| {
            if let Some(t) = Mthread::new(
                format!("mayastor_nvmf_tcp_pg_core_{}", r.core()),
                r.core(),
            ) {
                waiter.push(Self::create_poll_group(self.tgt.as_ptr(), t));
            }
        });

        futures::future::join_all(waiter).await;
    }

    async fn create_poll_group(tgt: *mut spdk_nvmf_tgt, mt: Mthread) {
        mt.with(|| {
            let pg = PollGroup::new(tgt, mt);
            NVMF_PGS.with(|p| p.borrow_mut().push(pg));
        });
    }

    pub async fn add_tcp_transport(&self) {
        let thread = NVMF_TARGET.get().unwrap().thread;

        thread.enter();
        let cfg = Config::get();
        let mut opts = cfg.nvmf_tcp_tgt_conf.opts.into();
        let transport = unsafe {
            spdk_nvmf_transport_create(
                "TCP".to_string().into_cstring().into_raw(),
                &mut opts,
            )
        };

        if transport.is_null() {
            panic!()
        }

        let (s, r) = futures::channel::oneshot::channel::<ErrnoResult<()>>();
        unsafe {
            spdk_nvmf_tgt_add_transport(
                self.tgt.as_ptr(),
                transport,
                Some(done_errno_cb),
                cb_arg(s),
            )
        };

        let _result = r.await.unwrap();
        debug!("Added TCP nvmf transport");
    }

    fn listen(&self) {
        let cfg = Config::get();
        let trid_nexus = TransportId::new(cfg.nexus_opts.nvmf_nexus_port);
        let mut opts = spdk_nvmf_listen_opts::default();
        unsafe {
            spdk_nvmf_listen_opts_init(
                &mut opts,
                std::mem::size_of::<spdk_nvmf_listen_opts>() as u64,
            );
        }
        let rc = unsafe {
            spdk_nvmf_tgt_listen_ext(
                self.tgt.as_ptr(),
                trid_nexus.as_ptr(),
                &mut opts,
            )
        };

        if rc != 0 {
            panic!("failed to create target");
        }

        let trid_replica = TransportId::new(cfg.nexus_opts.nvmf_replica_port);
        let rc = unsafe {
            spdk_nvmf_tgt_listen_ext(
                self.tgt.as_ptr(),
                trid_replica.as_ptr(),
                &mut opts,
            )
        };

        if rc != 0 {
            panic!("failed to create target");
        }
        info!(
            "nvmf target listening on {}:({},{})",
            get_ipv4_address().unwrap(),
            trid_nexus.trsvcid.as_str(),
            trid_replica.trsvcid.as_str(),
        );
    }
}

#[derive(Debug)]
pub struct Service {
    name: String,
    core: Cores,
    thread: Mthread,
    tgt: NvmfTgt,
}

//
// # Safety:
//
//  ffi struct is protected with a mutex
//
unsafe impl Sync for Service {}
unsafe impl Send for Service {}

use once_cell::sync::OnceCell;

pub static NVMF_TARGET: OnceCell<Service> = OnceCell::new();

impl Service {
    pub fn new(name: String, core: Cores) -> &'static Service {
        let thread = Mthread::new(name.clone(), core.id())
            .expect("failed to create service");
        thread.enter();
        let service = Self {
            name,
            core,
            thread,
            tgt: NvmfTgt::new(),
        };

        thread.exit();

        NVMF_TARGET.get_or_init(|| service)
    }

    pub async fn start(&self) {
        self.tgt.init_poll_groups().await;
        self.tgt.add_tcp_transport().await;
        self.tgt.listen();
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

    pub fn with_cb<F, C, R>(&self, f: F, callback: C)
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
                self.thread.exit();
                unsafe {
                    ManuallyDrop::drop(&mut self.inner);
                }
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
