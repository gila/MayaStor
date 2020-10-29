#[macro_use]
extern crate tracing;

use mayastor::{
    bdev::util::uring,
    core::{MayastorCliArgs, MayastorEnvironment, Reactors},
    grpc,
    logger,
    subsys,
};
use std::path::Path;
use structopt::StructOpt;
mayastor::CPS_INIT!();
use smol::{future, io, net, prelude::*, LocalExecutor, Unblock};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = MayastorCliArgs::from_args();

    let local_ex = LocalExecutor::new();

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

    info!("Starting Mayastor ..");
    info!(
        "kernel io_uring support: {}",
        if uring_supported { "yes" } else { "no" }
    );
    info!("free_pages: {} nr_pages: {}", free_pages, nr_pages);

    let grpc_endpoint = grpc::endpoint(args.grpc_endpoint.clone());
    let rpc_address = args.rpc_address.clone();

    let ms = MayastorEnvironment::new(args).init();
    let master = Reactors::master();
    master.send_future(async { info!("Mayastor started {} ...", '\u{1F680}') });
    master
        .spawn_local(async_compat::Compat::new(grpc::MayastorGrpcServer::run(
            grpc_endpoint,
            rpc_address,
        )))
        .detach();

    Reactors::launch_master();
    //let mut futures = Vec::new();

    //
    // futures.push(master.boxed_local());
    // futures.push(subsys::Registration::run().boxed_local());
    // futures.push(
    // );
    //
    // future::block_on(local_ex.run(futures::future::try_join_all(futures)))
    //     .unwrap();

    ms.fini();
    Ok(())
}
