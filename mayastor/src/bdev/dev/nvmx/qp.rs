mod opts {
    use crate::bdev::dev::nvmx::NvmeController;
    use spdk_sys::{
        spdk_nvme_ctrlr,
        spdk_nvme_ctrlr_get_default_io_qpair_opts,
        spdk_nvme_io_qpair_opts,
    };
    use std::mem::size_of;

    pub struct QPairOpts(spdk_nvme_io_qpair_opts);

    pub struct Builder {
        create_only: Option<bool>,
        /// The number of requests to allocate for this NVMe I/O queue.
        /// Overrides spdk_nvme_ctrlr_opts::io_queue_requests.
        /// This should be at least as large as io_queue_size.
        io_queue_requests: Option<u32>,
        /// The queue depth of this NVMe I/O queue. Overrides
        /// spdk_nvme_ctrlr_opts::io_queue_size.
        io_queue_size: Option<u32>,
        ///
        delay_cmd: Option<bool>,
        ctrlr: *mut spdk_nvme_ctrlr,
    }

    impl Builder {
        pub fn new() -> Self {
            Self {
                create_only: None,
                ctrlr: std::ptr::null_mut(),
                io_queue_size: None,
                io_queue_requests: None,
                delay_cmd: None,
            }
        }

        pub fn create_only(mut self, create_only: bool) -> Self {
            self.create_only = Some(create_only);
            self
        }

        pub fn with_io_queue_requests(
            mut self,
            io_queue_requests: u32,
        ) -> Self {
            self.io_queue_requests = Some(io_queue_requests);
            self
        }

        pub fn with_io_queue_size(mut self, io_queue_size: u32) -> Self {
            self.io_queue_size = Some(io_queue_size);
            self
        }

        pub fn with_controller(mut self, ctrlr: &NvmeController) -> Self {
            self.ctrlr = ctrlr.ctrlr_as_ptr();
            self
        }

        pub fn build(self) -> QPairOpts {
            if self.ctrlr.is_null() {
                panic!("qpairs can not be created with out a controller")
            }

            let mut opts = unsafe { spdk_nvme_io_qpair_opts::default() };

            unsafe {
                spdk_nvme_ctrlr_get_default_io_qpair_opts(
                    self.ctrlr,
                    &mut opts,
                    size_of::<spdk_nvme_io_qpair_opts>() as u64,
                );
            };

            if let Some(create_only) = self.create_only {
                opts.create_only = create_only;
            }

            if let Some(io_queue_requests) = self.io_queue_requests {
                opts.io_queue_requests = io_queue_requests;
            }

            if let Some(io_queue_size) = self.io_queue_size {
                opts.io_queue_size = io_queue_size;
            }

            QPairOpts(opts)
        }
    }
}

use spdk_sys::{spdk_nvme_io_qpair_opts, spdk_nvme_qpair};
use std::ptr::NonNull;
pub struct QPair(NonNull<spdk_nvme_qpair>);

/// FFI not available in header files
extern "C" {
    fn nvme_qpair_check_enabled(qp: *mut spdk_nvme_qpair) -> bool;
    fn nvme_qpair_abort_reqs(qp: *mut spdk_nvme_qpair, dnr: i32);
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum QPairState {
    Disconneted = 0,
    Disconneting = 1,
    Conneting = 2,
    Connected = 3,
    Enabeling = 4,
    Enabled = 5,
    Destroyed = 6,
}

pub enum Retry {
    DO = 0,
    DONT = 1,
}

impl From<Retry> for u32 {
    fn from(r: Retry) -> Self {
        match r {
            Retry::DO => 0,
            Retry::DONT => 1,
        }
    }
}

impl QPair {
    /// disconnect an IO qpair
    pub fn disconnect(&self) {}
    /// free an IO qpair if 0 == success, <=0 is failure
    pub fn free(&self) -> i32 {
        0
    }
    /// abort all request associated with this qp
    pub fn abort(&self, retry: Retry) {
        match retry {
            Retry::DO => unsafe { nvme_qpair_abort_reqs(self.0.as_ptr(), 0) },
            Retry::DONT => unsafe { nvme_qpair_abort_reqs(self.0.as_ptr(), 1) },
        }
    }
    /// validate the QP is usable for submitting IO
    pub fn enabled(&self) -> bool {
        unsafe { nvme_qpair_check_enabled(self.0.as_ptr()) }
    }
}
