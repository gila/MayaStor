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
    spdk_subsystem,
    spdk_subsystem_depend,
    spdk_subsystem_fini_next,
    spdk_subsystem_init_next,
};

pub use mbus::{
    mbus_endpoint,
    message_bus_init,
    registration::Registration,
    MessageBusSubsystem,
};

use crate::{
    ffihelper::IntoCString,
    subsys::{child::ChildSubsystem, nvmf::Nvmf},
};
use std::fmt::Display;

pub mod child;
mod config;
mod mbus;
mod nvmf;

pub trait Subsystem {
    extern "C" fn init() {
        //TODO: make this call implicit by taking a closure
        unsafe { spdk_subsystem_init_next(0) }
    }

    extern "C" fn fini() {
        unsafe { spdk_subsystem_fini_next() }
    }

    fn new<N: Into<String> + Display>(name: N) {
        let mut ss = Box::new(spdk_subsystem::default());
        ss.name = name.to_string().into_cstring().into_raw();
        ss.init = Some(Self::init);
        ss.fini = Some(Self::fini);
        ss.write_config_json = None;
        unsafe { spdk_add_subsystem(Box::into_raw(ss)) };
    }
}

struct SubsystemDependency {
    inner: Box<spdk_sys::spdk_subsystem_depend>,
}

impl SubsystemDependency {
    pub fn new<N: Into<String> + Display>(name: N, depends: N) {
        let mut inner = Box::new(spdk_sys::spdk_subsystem_depend::default());

        inner.name = name.to_string().into_cstring().into_raw();
        inner.depends_on = depends.to_string().into_cstring().into_raw();
        Self {
            inner,
        }
        .depend();
    }

    fn depend(self) {
        unsafe { spdk_add_subsystem_depend(Box::into_raw(self.inner)) };
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

    ChildSubsystem::new("child_subsystem");
    SubsystemDependency::new("child_subsystem", "bdev");
    MessageBusSubsystem::register();
}
