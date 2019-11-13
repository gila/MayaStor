use std::env;
use std::ffi::CString;
use std::process::Command;

use spdk_sys::{spdk_app_opts, spdk_app_opts_init};

pub fn mayastor_test_init() {
    let log = mayastor::spdklog::SpdkLog::new();
    let _ = log.init();
    env::set_var("MAYASTOR_LOGLEVEL", "4");
    mayastor::CPS_INIT!();
}

pub fn create_disk(path: &str, size: &str) {
    let output = Command::new("truncate")
        .args(&["-s", size, path])
        .output()
        .expect("failed exec truncate");

    assert_eq!(output.status.success(), true);
}

pub fn delete_disk(disks: &[String]) {
    let output = Command::new("rm")
        .args(&["-rf"])
        .args(disks)
        .output()
        .expect("failed delete test file");

    assert_eq!(output.status.success(), true);
}
