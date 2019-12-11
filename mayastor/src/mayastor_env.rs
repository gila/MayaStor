use std::{ffi::CString, os::raw::c_void};

use nix::sys::{
    signal,
    signal::{
        pthread_sigmask,
        SigHandler,
        SigSet,
        SigmaskHow,
        Signal::{SIGINT, SIGTERM},
    },
};
use snafu::{ensure, Backtrace, ErrorCompat, ResultExt, Snafu};
use structopt::StructOpt;

use spdk_sys::{
    maya_log,
    spdk_app_shutdown_cb,
    spdk_app_stop,
    spdk_conf_allocate,
    spdk_conf_free,
    spdk_conf_read,
    spdk_conf_set_as_default,
    spdk_cpuset_alloc,
    spdk_cpuset_free,
    spdk_cpuset_set_cpu,
    spdk_cpuset_zero,
    spdk_env_get_core_count,
    spdk_env_get_current_core,
    spdk_log_level,
    spdk_log_open,
    spdk_log_set_level,
    spdk_log_set_print_level,
    spdk_pci_addr,
    spdk_rpc_set_state,
    spdk_thread_create,
    spdk_thread_send_msg,
    SPDK_LOG_DEBUG,
    SPDK_LOG_INFO,
    SPDK_RPC_RUNTIME,
};

use crate::{app_start_cb, app_start_motor, event::Mthread, log_impl};

extern "C" {
    pub fn rte_eal_init(argc: i32, argv: *mut *mut libc::c_char) -> i32;
    pub fn rte_log_set_level(_type: i32, level: i32) -> i32;
    pub fn spdk_reactors_init() -> i32;
    pub fn spdk_reactors_start();
    pub fn spdk_reactors_stop() -> libc::c_void;
    pub fn bootstrap_fn(arg: *mut libc::c_void);
    pub fn spdk_subsystem_init(
        f: extern "C" fn(i32, *mut c_void),
        ctx: *mut c_void,
    );
    pub fn spdk_rpc_initialize(listen: *mut libc::c_char);
}

#[derive(Debug, Snafu)]
pub enum EnvError {
    #[snafu(display("Failed to install signal handler"))]
    SetSigHdl { source: nix::Error },
    #[snafu(display("Failed to read configuration file: {}", reason))]
    ParseConfig { reason: String },
}

type Result<T, E = EnvError> = std::result::Result<T, E>;

#[derive(Debug, Default, StructOpt)]
#[structopt(
    name = "mayastor",
    about = "Containerized Attached Storage (CAS) for k8s",
    raw(setting = "structopt::clap::AppSettings::ColoredHelp")
)]

pub struct MayastorCliArgs {
    #[structopt(short = "m", default_value = "0x1")]
    reactor_mask: String,
    #[structopt(short = "u")]
    no_pci: bool,
    #[structopt(short = "L")]
    log_flag: Vec<String>,
}

/// Mayastor argument
#[derive(Debug)]
pub struct MayastorOpt {
    pub config_file: String,
    delay_subsystem_init: bool,
    enable_coredump: bool,
    env_context: String,
    hugedir: String,
    hugepage_single_segments: bool,
    json_config_file: String,
    master_core: i32,
    mem_channel: i32,
    mem_size: i32,
    name: String,
    no_pci: bool,
    num_entries: u64,
    num_pci_addr: usize,
    pci_blacklist: Vec<spdk_pci_addr>,
    pci_whitelist: Vec<spdk_pci_addr>,
    print_level: spdk_log_level,
    reactor_mask: String,
    rpc_addr: String,
    shm_id: i32,
    shutdown_cb: spdk_app_shutdown_cb,
    tpoint_group_mask: String,
    unlink_hugepage: bool,
}

impl Default for MayastorOpt {
    fn default() -> Self {
        Self {
            config_file: String::new(),
            delay_subsystem_init: false,
            enable_coredump: true,
            env_context: String::new(),
            hugedir: String::new(),
            hugepage_single_segments: false,
            json_config_file: "".to_string(),
            master_core: -1,
            mem_channel: -1,
            mem_size: -1,
            name: "mayastor".into(),
            no_pci: false,
            num_entries: 32 * 1024,
            num_pci_addr: 0,
            pci_blacklist: vec![],
            pci_whitelist: vec![],
            print_level: SPDK_LOG_INFO,
            reactor_mask: "0x1".into(),
            rpc_addr: "/var/tmp/mayastor.sock".into(),
            shm_id: -1,
            shutdown_cb: None,
            tpoint_group_mask: String::new(),
            unlink_hugepage: false,
        }
    }
}

/// called on SIGINT and SIGTERM
extern "C" fn signal_handler(signo: i32) {
    warn!("Received signo {}, shutting down", signo);
    unsafe {
        spdk_app_stop(0);
    }
    //unsafe { spdk_reactors_stop() };
}

impl MayastorOpt {
    /// configure signal handling
    pub fn install_signal_handlers(&self) -> Result<()> {
        // first set that we ignore SIGPIPE
        let _ = unsafe { signal::signal(signal::SIGPIPE, SigHandler::SigIgn) }
            .context(SetSigHdl)?;

        // setup that we want signal_handler to be invoked on SIGINT and SIGTERM
        let handler = SigHandler::Handler(signal_handler);
        unsafe {
            signal::signal(SIGINT, handler).context(SetSigHdl)?;
            signal::signal(SIGTERM, handler).context(SetSigHdl)?;
        }

        let mut mask = SigSet::empty();
        mask.add(SIGINT);
        mask.add(SIGTERM);
        pthread_sigmask(SigmaskHow::SIG_UNBLOCK, Some(&mask), None)
            .context(SetSigHdl)?;

        Ok(())
    }

    /// read the config file we use this mostly for testing
    pub fn read_config_file(&self) -> Result<()> {
        let path = CString::new(self.config_file.as_str()).unwrap();
        let config = unsafe { spdk_conf_allocate() };

        assert_ne!(config, std::ptr::null_mut());

        if unsafe { spdk_conf_read(config, path.as_ptr()) } != 0 {
            return Err(EnvError::ParseConfig {
                reason: "Failed to read file from disk".into(),
            });
        }

        let rc = unsafe {
            if spdk_sys::spdk_conf_first_section(config).is_null() {
                Err(EnvError::ParseConfig {
                    reason: "failed to parse config file".into(),
                })
            } else {
                Ok(())
            }
        };

        if rc.is_ok() {
            trace!("Setting default config to {:p}", config);
            unsafe { spdk_conf_set_as_default(config) };
        } else {
            unsafe { spdk_conf_free(config) }
        }

        rc
    }

    /// construct an array of options to be passed to EAL and start it
    pub fn start_eal(&self) {
        let mut args: Vec<CString> = Vec::new();

        args.push(CString::new(self.name.clone()).unwrap());

        args.push(CString::new(format!("-c {}", self.reactor_mask)).unwrap());

        if self.mem_channel > 0 {
            args.push(
                CString::new(format!("-n {}", self.mem_channel)).unwrap(),
            );
        }

        if self.master_core > 0 {
            args.push(
                CString::new(format!("--master-lcore={}", self.master_core))
                    .unwrap(),
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

        if !self.hugedir.is_empty() {
            args.push(
                CString::new(format!("--huge-dir={}", self.hugedir)).unwrap(),
            )
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
        // set the log levels of the DPDK libs can be overridden by setting
        // env_context
        args.push(CString::new("--log-level=lib.eal:4").unwrap());
        args.push(CString::new("--log-level=lib.cryptodev:0").unwrap());
        args.push(CString::new("--log-level=user1:6").unwrap());

        if !self.env_context.is_empty() {
            args.push(CString::new(self.env_context.clone()).unwrap());
        }

        debug!("EAL args: {:?}", args);

        let mut cargs = args
            .iter()
            .map(|arg| arg.as_ptr())
            .collect::<Vec<*const i8>>();
        cargs.push(std::ptr::null());

        unsafe { rte_log_set_level(0, 8) };
        if unsafe {
            rte_eal_init(
                (cargs.len() as libc::c_int) - 1,
                cargs.as_ptr() as *mut *mut i8,
            )
        } < 0
        {
            panic!("Failed to init EAL");
        }
    }

    //    extern "C" fn subsystem_callback(rc: i32, ctx: *mut c_void) {
    //        return;
    //        //        unsafe {
    //        //            let margs = unsafe { &*(ctx as *mut MayastorOpt) };
    //        //            let rpc =
    // CString::new(margs.rpc_addr.as_str()).unwrap();        //
    // if rc != 0 {        //                spdk_app_stop(rc);
    //        //                return;
    //        //            }
    //        //            spdk_rpc_initialize(rpc.as_ptr() as *mut i8);
    //        //            spdk_rpc_set_state(SPDK_RPC_RUNTIME);
    //        //
    //        //            app_start_motor(ctx);
    //        //        }
    //    }
    //
    //    extern "C" fn bootstrap(arg: *mut c_void) {
    //        unsafe { spdk_subsystem_init(Self::subsystem_callback, arg) }
    //    }

    pub fn start(&self) -> Result<()> {
        if self.enable_coredump {
            warn!("rlimit configuration not implemented");
        }

        unsafe {
            spdk_log_set_level(SPDK_LOG_DEBUG);
            spdk_log_set_print_level(SPDK_LOG_DEBUG);
            spdk_log_open(Some(maya_log));
            spdk_sys::logfn = Some(log_impl);
        }

        info!("Total number of cores available: {}", unsafe {
            spdk_env_get_core_count()
        });

        let rc = unsafe { spdk_reactors_init() };

        if rc != 0 {
            error!("Failed to initialize reactors, there is no point to continue, error code: {}", rc);
            std::process::exit(rc);
        }

        let cpu_mask = unsafe { spdk_cpuset_alloc() };

        if cpu_mask.is_null() {
            error!("CPU set allocation failed");
            std::process::exit(1);
        }

        unsafe {
            spdk_cpuset_zero(cpu_mask);
            spdk_cpuset_set_cpu(cpu_mask, spdk_env_get_current_core(), true);
            spdk_cpuset_free(cpu_mask);
        }

        let thread = {
            let name = CString::new("mayastor_master_thread").unwrap();
            unsafe { spdk_thread_create(name.as_ptr(), cpu_mask) }
        };

        if thread.is_null() {
            error!("Failed to allocate the main thread");
            std::process::exit(1);
        }

        self.install_signal_handlers()?;

        Mthread(thread).with(|| unsafe {
            spdk_subsystem_init(
                Self::subsystem_callback,
                *&self as *const _ as *mut _,
            );
        });

        Mthread(thread).with(|| unsafe {
            let rpc = CString::new(self.rpc_addr.as_str()).unwrap();
            if rc != 0 {
                spdk_app_stop(rc);
                return;
            }
            spdk_rpc_initialize(rpc.as_ptr() as *mut i8);
            spdk_rpc_set_state(SPDK_RPC_RUNTIME);
        });

        //        unsafe {
        //            spdk_thread_send_msg(
        //                thread,
        //                Some(Self::bootstrap),
        //                &*self as *const _ as *mut _,
        //            );
        //
        //            spdk_reactors_start();
        //        }

        Ok(())
    }
}
