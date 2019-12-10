use spdk_sys::{
    __log_init,
    spdk_app_shutdown_cb,
    spdk_cpuset_alloc,
    spdk_cpuset_set_cpu,
    spdk_cpuset_zero,
    spdk_env_dpdk_post_init,
    spdk_env_get_current_core,
    spdk_log_level,
    spdk_log_set_backtrace_level,
    spdk_log_set_level,
    spdk_log_set_print_level,
    spdk_pci_addr,
    spdk_sighandler_t,
    spdk_thread_create,
    spdk_thread_send_msg,
    SPDK_LOG_INFO,
};

use super::spdklog::SpdkLog;

extern "C" {
    pub fn rte_eal_init(argc: i32, argv: *mut *mut libc::c_char) -> i32;
    pub fn rte_log_set_level(_type: i32, level: i32) -> i32;
    pub fn spdk_reactors_init() -> i32;
    pub fn spdk_reactors_start();
    pub fn spdk_reactors_stop() -> libc::c_void;
    pub fn bootstrap_fn(arg: *mut libc::c_void);
}

//extern "C" {
//    static mut logfn: Option<
//        extern "C" fn(
//            level: u32,
//            file: *mut char,
//            line: u32,
//            func: *mut char,
//            buf: *mut char,
//        ),
//    >;
//}

use std::{
    ffi::{CStr, CString},
    os::raw::{c_char, c_void},
};
use va_list::VaList;

const RTE_LOGTYPE_EAL: i32 = 0;
const RTE_LOG_NOTICE: i32 = 1;
const RTE_LOG_DEBUG: i32 = 8;
const SPDK_APP_DPDK_DEFAULT_MEM_SIZE: i32 = -1;
const SPDK_APP_DPDK_DEFAULT_MASTER_CORE: i32 = -1;
const SPDK_APP_DPDK_DEFAULT_MEM_CHANNEL: i32 = -1;
const SPDK_APP_DPDK_DEFAULT_CORE_MASK: &str = "0x1";
const SPDK_APP_DEFAULT_CORE_LIMIT: u64 = 0x140000000; /* 5 GiB */

#[repr(C)]
#[derive(Debug, Clone)]
pub struct MsAppOpts {
    pub config_file: String,
    pub delay_subsystem_init: bool,
    pub enable_coredump: bool,
    pub hugedir: String,
    pub hugepage_single_segments: bool,
    pub json_config_file: *const libc::c_char,
    pub master_core: i32,
    pub max_delay_us: u64,
    pub mem_channel: libc::c_int,
    pub mem_size: i32,
    pub name: String,
    pub no_pci: bool,
    pub num_entries: u64,
    pub num_pci_addr: usize,
    pub pci_blacklist: *mut spdk_pci_addr,
    pub pci_whitelist: *mut spdk_pci_addr,
    pub print_level: spdk_log_level,
    pub reactor_mask: String,
    pub rpc_addr: String,
    pub shm_id: i32,
    pub shutdown_cb: spdk_app_shutdown_cb,
    pub tpoint_group_mask: *const libc::c_char,
    pub unlink_hugepage: bool,
    pub usr1_handler: spdk_sighandler_t,
    default_config: *mut spdk_sys::spdk_conf,
}

impl Default for MsAppOpts {
    fn default() -> Self {
        unsafe { ::std::mem::zeroed() }
    }
}

impl MsAppOpts {
    fn new() -> MsAppOpts {
        let mut opts = MsAppOpts::default();
        opts.delay_subsystem_init = false;
        opts.enable_coredump = true;
        opts.master_core = SPDK_APP_DPDK_DEFAULT_MASTER_CORE;
        opts.mem_channel = SPDK_APP_DPDK_DEFAULT_MEM_CHANNEL;
        opts.mem_size = SPDK_APP_DPDK_DEFAULT_MEM_SIZE;
        opts.name = "MayaStor".into();
        opts.num_entries = 32 * 1024;
        opts.print_level = SPDK_LOG_INFO;
        opts.reactor_mask = SPDK_APP_DPDK_DEFAULT_CORE_MASK.into();
        opts.rpc_addr = "/var/tmp/mayastor.sock".into();
        opts.shm_id = -1;

        //        opts.config_file = "/code/spdk-nvme.conf".into();

        opts
    }

    /// we need to convert some option values to argv** argc to passs to EAL
    fn to_env_opts(&self) -> Vec<*const i8> {
        let mut args: Vec<CString> = Vec::new();

        args.push(CString::new(self.name.clone()).unwrap());

        let mask = CString::new(format!("-c {}", self.reactor_mask)).unwrap();

        dbg!(&mask);

        if self.mem_channel > 0 {
            args.push(
                CString::new(format!("-n {}", self.mem_channel)).unwrap(),
            );
        }

        if self.shm_id < 0 {
            args.push(CString::new("--no-shconf").unwrap());
        }

        if self.mem_size >= 0 {
            args.push(CString::new(format!("-m {}", self.mem_size)).unwrap());
        }

        if self.master_core > 0 {
            args.push(
                CString::new(format!("--master-lcore={}", self.master_core))
                    .unwrap(),
            );
        }

        if self.no_pci {
            args.push(CString::new("--no-pci").unwrap());
        }

        if self.hugepage_single_segments {
            args.push(CString::new("--single-file-segments").unwrap());
        }

        if !self.hugedir.is_empty() || self.num_pci_addr != 0 {
            dbg!("hugedir and pci black/wite list implemented");
        }

        if cfg!(target_os = "linux") {
            // Ref: https://github.com/google/sanitizers/wiki/AddressSanitizerAlgorithm
            args.push(CString::new("--base-virtaddr=0x200000000000").unwrap());
        }

        if self.shm_id < 0 {
            args.push(
                CString::new(format!("--file-prefix=mayastor_pid{}", unsafe {
                    libc::getpid()
                }))
                .unwrap(),
            );
        } else {
            args.push(
                CString::new(format!(
                    "--file-prefix=mayastor_pid{}",
                    self.shm_id
                ))
                .unwrap(),
            );
            args.push(CString::new("--proc-type=auto").unwrap());
        }

        let mut cargs = args
            .iter()
            .map(|arg| arg.as_ptr())
            .collect::<Vec<*const i8>>();
        cargs.push(std::ptr::null());
        cargs
    }

    fn install_signal_handlers(&self) {
        if self.shutdown_cb == None {
            dbg!("custom shutdown_cb not supported yet..");
        }
        let mut sigact = unsafe {
            libc::sigaction {
                ..::std::mem::zeroed()
            }
        };

        let mut sigset: libc::sigset_t = unsafe { std::mem::uninitialized() };

        unsafe {
            libc::sigemptyset(&mut sigset);
            libc::sigemptyset(&mut sigact.sa_mask);
        };

        sigact.sa_sigaction = libc::SIG_IGN;
        let rc = unsafe {
            libc::sigaction(libc::SIGPIPE, &sigact, ::std::ptr::null_mut())
        };

        if rc < 0 {
            println!("failed to install SIGPIPE signal handler!")
        }

        sigact.sa_sigaction = shutdown_signal as usize;

        let _ = [libc::SIGTERM, libc::SIGINT]
            .iter()
            .map(|sig| {
                let rc = unsafe {
                    libc::sigaction(*sig, &sigact, ::std::ptr::null_mut())
                };
                if rc != 0 {
                    dbg!("failed to install signal handler of sig");
                    dbg!(*sig);
                }

                rc
            })
            .collect::<Vec<_>>();
    }

    fn read_config_file(&mut self) {
        let config = unsafe { spdk_sys::spdk_conf_allocate() };
        let rc = unsafe {
            spdk_sys::spdk_conf_read(
                config,
                CString::new(self.config_file.clone()).unwrap().as_ptr(),
            )
        };

        if rc != 0 {
            println!("error reading config file")
        }

        let rc: i32 = unsafe {
            if spdk_sys::spdk_conf_first_section(config) == std::ptr::null_mut()
            {
                println!("error parsing config file... illformated");
                1
            } else {
                0
            }
        };

        if rc != 0 {
            unsafe { spdk_sys::spdk_conf_free(config) };
        }

        self.default_config = config;
    }
}

/// make Result()
/// move to impl of app?
pub fn eal_init(opts: &MsAppOpts) -> i32 {
    println!("Initializing Mayastor v0.01");

    let env_opts = opts.to_env_opts();

    let rc = unsafe { rte_log_set_level(RTE_LOGTYPE_EAL, RTE_LOG_DEBUG) };
    let rc = unsafe {
        rte_eal_init(
            (env_opts.len() as libc::c_int) - 1,
            env_opts.as_ptr() as *mut *mut i8,
        )
    };

    if rc != 0 {
        println!("oh shit nothing works!");
        return -1;
    }

    let rc = unsafe { rte_log_set_level(RTE_LOGTYPE_EAL, RTE_LOG_NOTICE) };

    if rc != 0 {
        println!("failed to set EAL log level");
    }

    let rc = unsafe { spdk_env_dpdk_post_init() };
    rc
}

// TODO: coredumps
//
pub fn mayastor_start() -> i32 {
    // there are varios checks that fix behaviour of older DPDK's
    // we dont want to inherit that so assert DPDK version here
    //

    //    let log = SpdkLog::new();
    //    log.init().expect("Failed to set logger");

    let mut app_opts = MsAppOpts::new();
    let cargs = app_opts.to_env_opts();
    dbg!(&cargs);
    dbg!(&app_opts);
    unsafe {
        spdk_log_set_print_level(app_opts.print_level);
        spdk_log_set_level(app_opts.print_level);
        spdk_log_set_backtrace_level(0);
    };

    app_opts.install_signal_handlers();
    app_opts.read_config_file();

    let rc = eal_init(&app_opts);

    let rc = unsafe { spdk_reactors_init() };
    if rc != 0 {
        error!("kaput..");
    }

    let mut cpu_mask = unsafe { spdk_cpuset_alloc() };

    unsafe {
        spdk_cpuset_zero(cpu_mask);
        spdk_cpuset_set_cpu(cpu_mask, spdk_env_get_current_core(), true);
    }

    extern "C" fn logg_fn(
        level: u32,
        file: *const c_char,
        line: u32,
        func: *const c_char,
        buf: *const c_char,
        n: i32,
    ) {
        unsafe {
            eprintln!(
                "{}, {}, {}, {} {}",
                level,
                CStr::from_ptr(file).to_str().unwrap(),
                line,
                CStr::from_ptr(func).to_str().unwrap(),
                CStr::from_ptr(buf).to_str().unwrap()
            );
        }
    }
    unsafe {
        __log_init(Some(logg_fn));
    }

    info!("mayastor started");

    let thread = unsafe {
        let name = CString::new("maya_master").unwrap();

        spdk_thread_create(name.as_ptr(), cpu_mask)
    };

    if thread.is_null() {
        panic!("no main thread");
    }

    extern "C" fn bootstrapper(arg: *mut libc::c_void) {
        info!("bootstrapping");
    }

    unsafe {
        spdk_thread_send_msg(thread, Some(bootstrapper), std::ptr::null_mut());
        spdk_reactors_start();
    }

    0
}

pub extern "C" fn shutdown_signal(signo: i32) -> *mut libc::c_void {
    println!("signal recieved: {} ... good!", signo);

    unsafe { spdk_reactors_stop() };

    std::ptr::null_mut()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_ms_appt_opts() {
        mayastor_start();
    }
}
