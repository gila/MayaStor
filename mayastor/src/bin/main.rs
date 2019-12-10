extern crate git_version;
extern crate libc;
#[macro_use]
extern crate log;

use std::path::Path;

use git_version::git_version;

use mayastor::{mayastor_logger_init, mayastor_start};
use sysfs;

mayastor::CPS_INIT!();

fn main() -> Result<(), std::io::Error> {
    let hugepage_path = Path::new("/sys/kernel/mm/hugepages/hugepages-2048kB");
    let nr_pages: u32 = sysfs::parse_value(&hugepage_path, "nr_hugepages")?;

    if nr_pages == 0 {
        warn!("no hugepages available, allocating 512 default pages of 2048k");
        sysfs::write_value(&hugepage_path, "nr_hugepages", 512)?;
    }

    let free_pages: u32 = sysfs::parse_value(&hugepage_path, "free_hugepages")?;
    let nr_pages: u32 = sysfs::parse_value(&hugepage_path, "nr_hugepages")?;

    info!("free_pages: {} nr_pages: {}", free_pages, nr_pages);

    mayastor_logger_init("INFO");
    mayastor_start("Mayastor", std::env::args().collect(), || {
        info!("Mayastor started {} ({})...", '\u{1F680}', git_version!());
    });

    Ok(())
}
