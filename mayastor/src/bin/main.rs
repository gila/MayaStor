#[macro_use]
extern crate log;

use git_version::git_version;
use std::path::Path;
use structopt::StructOpt;

use mayastor::{
    bdev::util::uring,
    core::{MayastorCliArgs, MayastorEnvironment},
    logger,
};

mayastor::CPS_INIT!();

fn main() -> Result<(), std::io::Error> {
    let mut version = git_version!(fallback = "NO_GIT");

    let env_version =
        std::env::var("GIT_VERSION").unwrap_or_else(|_| "UNKNOWN".into());

    if version == "NO_GIT" {
        version = &env_version;
    }

    let args = MayastorCliArgs::from_args();

    // setup our logger first if -L is passed, raise the log level
    // automatically. trace maps to debug at FFI level. If RUST_LOG is
    // passed, we will use it regardless.

    if !args.log_components.is_empty() {
        logger::init("TRACE");
    } else {
        logger::init("INFO");
    }

    let hugepage_path = Path::new("/sys/kernel/mm/hugepages/hugepages-2048kB");
    let nr_pages: u32 = sysfs::parse_value(&hugepage_path, "nr_hugepages")?;

    if nr_pages == 0 {
        warn!("No hugepages available, trying to allocate 512 2MB hugepages");
        sysfs::write_value(&hugepage_path, "nr_hugepages", 512)?;
    }

    let free_pages: u32 = sysfs::parse_value(&hugepage_path, "free_hugepages")?;
    let nr_pages: u32 = sysfs::parse_value(&hugepage_path, "nr_hugepages")?;
    let uring_supported = uring::kernel_support();

    info!(
        "Starting Mayastor ({}) PID: {}",
        version,
        std::process::id()
    );
    info!(
        "kernel io_uring support: {}",
        if uring_supported { "yes" } else { "no" }
    );
    info!("free_pages: {} nr_pages: {}", free_pages, nr_pages);
    let env = MayastorEnvironment::new(args);
    let exit = env
        .start(|| {
            info!("Mayastor started {} ...", '\u{1F680}');
        })
        .unwrap();

    if exit != 0 {
        warn!("mayastor exit code {}", exit);
        return Err(std::io::Error::from_raw_os_error(exit));
    }

    info!("mayastor shutdown completed, goodbye!");
    Ok(())
}
