mod job;
use job::JobList;

use std::{cell::RefCell, os::raw::c_void, ptr::NonNull};

use clap::{value_t, App, AppSettings, Arg};

use crate::job::Job;
use mayastor::{
    core::{
        Bdev,
        Cores,
        Descriptor,
        DmaBuf,
        IoChannel,
        MayastorCliArgs,
        MayastorEnvironment,
        Mthread,
        Reactors,
    },
    logger,
    nexus_uri::bdev_create,
    subsys::Config,
};
use spdk_sys::{spdk_poller, spdk_poller_unregister};

#[derive(Debug)]
enum IoType {
    /// perform random read operations
    READ,
    /// perform random write operations
    #[allow(dead_code)]
    WRITE,
}

thread_local! {
    static PERF_TICK: RefCell<Option<NonNull<spdk_poller>>> = RefCell::new(None);
}
/// default queue depth
const QD: u64 = 64;
/// default io_size
const IO_SIZE: u64 = 512;

/// override the default signal handler as we need to stop the jobs first
/// before we can shut down
fn sig_override() {
    let handler = || {
        Mthread::get_init().msg((), |_| {
            PERF_TICK.with(|t| {
                let ticker = t.borrow_mut().take().unwrap();
                unsafe { spdk_poller_unregister(&mut ticker.as_ptr()) }
            });

            println!("Draining jobs....");
            JobList::get().drain_all();
        });
    };

    unsafe {
        signal_hook::register(signal_hook::SIGTERM, handler)
            .expect("failed to set SIGTERM");
        signal_hook::register(signal_hook::SIGINT, handler)
            .expect("failed to set SIGINT");
    };
}

/// prints the performance statistics to stdout on every tick (1s)
extern "C" fn perf_tick(_: *mut c_void) -> i32 {
    JobList::get().stats()
}

fn main() {
    logger::init("INFO");

    // dont not start the target(s)
    Config::get_or_init(|| {
        let mut cfg = Config::default();
        cfg.nexus_opts.iscsi_enable = false;
        cfg.nexus_opts.nvmf_enable = false;
        cfg.sync_disable = true;
        cfg
    });

    let matches = App::new("\nMayastor performance tool")
        .version("0.1")
        .settings(&[AppSettings::ColoredHelp, AppSettings::ColorAlways])
        .about("Perform IO to storage URIs")
        .arg(
            Arg::with_name("io_size")
                .value_name("io_size")
                .short("b")
                .help("block size in bytes")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("queue_depth")
                .value_name("queue_depth")
                .short("q")
                .help("queue depth")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("URI")
                .value_name("URI")
                .help("storage URI's")
                .index(1)
                .multiple(true)
                .takes_value(true),
        )
        .get_matches();

    let mut uris = matches
        .values_of("URI")
        .unwrap()
        .map(|u| u.to_string())
        .collect::<Vec<_>>();

    let io_size = value_t!(matches.value_of("io_size"), u64).unwrap_or(IO_SIZE);
    let qd = value_t!(matches.value_of("queue_depth"), u64).unwrap_or(QD);
    let mut args = MayastorCliArgs::default();

    args.reactor_mask = "0x2".to_string();
    std::thread::spawn(move || {
        MayastorEnvironment::new(args).init();
        sig_override();
        Reactors::master().send_future(async move {
            println!(
                "current core: {} current CPU: {}",
                Cores::current(),
                Mthread::cpu_get_current()
            );
            let jobs = uris
                .iter_mut()
                .map(|u| Job::new(u, io_size, qd))
                .collect::<Vec<_>>();

            for j in jobs {
                let job = j.await;
                let thread =
                    Mthread::new(job.name().into(), Cores::current()).unwrap();
                thread.msg(job, |job| {
                    job.run();
                });
            }

            unsafe {
                PERF_TICK.with(|p| {
                    *p.borrow_mut() =
                        NonNull::new(spdk_sys::spdk_poller_register(
                            Some(perf_tick),
                            std::ptr::null_mut(),
                            1_000_000,
                        ))
                });
            }
        });

        Reactors::master().running();
        Reactors::master().poll_reactor();
    })
    .join()
    .unwrap();
}
