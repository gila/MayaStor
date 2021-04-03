use spdk_sys::spdk_subsystem;

mod inventory;

use crate::subsys::Subsystem;
pub use inventory::Inventory;

pub struct ChildSubsystem(*mut spdk_subsystem);

impl Subsystem for ChildSubsystem {
    extern "C" fn init() {
        debug!("hello?");
        unsafe { spdk_sys::spdk_subsystem_init_next(0) };
    }

    extern "C" fn fini() {
        Inventory::get().drop_all();
        unsafe { spdk_sys::spdk_subsystem_fini_next() };
    }
}
