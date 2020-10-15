#[macro_use]
extern crate tracing;

use crossbeam::crossbeam_channel::bounded;
use futures::FutureExt;
use mayastor::{
    bdev::util::uring,
    core::{MayastorCliArgs, MayastorEnvironment, Reactor, Reactors},
    grpc,
    logger,
    subsys,
};
use std::{net::SocketAddr, path::Path};
use structopt::StructOpt;
use tokio::prelude::*;
mayastor::CPS_INIT!();
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = MayastorCliArgs::from_args();

    let mut rt = tokio::runtime::Builder::new()
        .basic_scheduler()
        .enable_all()
        .build()
        .unwrap();

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

    let ms = rt.enter(|| MayastorEnvironment::new(args).init());

    let master = Reactors::master();
    let mut futures = Vec::new();

    futures.push(master.boxed_local());
    futures.push(subsys::Registration::run().boxed_local());
    futures.push(grpc::MayastorGrpcServer::run(grpc_endpoint).boxed_local());

    rt.block_on(futures::future::try_join_all(futures))
        .expect_err("reactor exit in abnormal state");

    ms.fini();
    Ok(())
}
