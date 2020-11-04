//! helper routines to drive IO to the nexus for testing purposes
use std::{
    ptr::NonNull,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use spdk_sys::{
    spdk_bdev_abort,
    spdk_bdev_free_io,
    spdk_bdev_read,
    spdk_bdev_reset,
    spdk_bdev_write,
};

use crate::{
    bdev::nexus::nexus_io::Bio,
    core::{Bdev, Descriptor, DmaBuf, IoChannel},
    nexus_uri::bdev_create,
};

#[derive(Debug)]
pub enum IoType {
    /// perform random read operations
    READ,
    /// perform random write operations
    WRITE,
}

impl Default for IoType {
    fn default() -> Self {
        Self::READ
    }
}

#[derive(Debug)]
struct Io {
    /// buffer we read/write from/to
    buf: DmaBuf,
    /// type of IO we are supposed to issue
    iot: IoType,
    /// current offset where we are reading from
    offset: u64,
    /// pointer to our the job we belong too
    job: NonNull<Job>,
}

impl Io {
    /// start submitting
    fn run(&mut self, job: *mut Job) {
        self.job = NonNull::new(job).unwrap();
        match self.iot {
            IoType::READ => self.read(0),
            IoType::WRITE => self.write(0),
        };
    }

    fn job(&mut self) -> &mut Job {
        unsafe { self.job.as_mut() }
    }

    /// dispatch the next IO, this is called from within the completion callback
    pub fn next(&mut self, offset: u64) {
        if self.job().request_reset {
            self.job().request_reset = false;
            self.reset();
            return;
        }

        if self.job().request_abort {
            self.job().request_abort = false;
            self.abort();
            return;
        }

        match self.iot {
            IoType::READ => self.read(offset),
            IoType::WRITE => self.write(offset),
        }
    }

    /// dispatch the read IO at given offset
    fn read(&mut self, offset: u64) {
        unsafe {
            if spdk_bdev_read(
                self.job.as_ref().desc.as_ptr(),
                self.job.as_ref().ch.as_ref().unwrap().as_ptr(),
                *self.buf,
                offset,
                self.buf.len(),
                Some(Job::io_completion),
                self as *const _ as *mut _,
            ) == 0
            {
                self.job.as_mut().n_inflight += 1;
            } else {
                eprintln!(
                    "failed to submit read IO to {}",
                    self.job.as_ref().bdev.name()
                );
            }
        };
    }

    /// dispatch write IO at given offset
    fn write(&mut self, offset: u64) {
        unsafe {
            if spdk_bdev_write(
                self.job.as_ref().desc.as_ptr(),
                self.job.as_ref().ch.as_ref().unwrap().as_ptr(),
                *self.buf,
                offset,
                self.buf.len(),
                Some(Job::io_completion),
                self as *const _ as *mut _,
            ) == 0
            {
                self.job.as_mut().n_inflight += 1;
            } else {
                eprintln!(
                    "failed to submit write IO to {}",
                    self.job.as_ref().bdev.name()
                );
            }
        };
    }

    pub fn reset(&mut self) {
        extern "C" fn reset_cb(
            bdev_io: *mut spdk_sys::spdk_bdev_io,
            success: bool,
            arg: *mut std::ffi::c_void,
        ) {
            if !success {
                eprintln!("failed to reset bdev");
            } else {
                println!("spdk_bdev reset successful");
            }
            unsafe { spdk_bdev_free_io(bdev_io) };
        }

        unsafe {
            if spdk_bdev_reset(
                self.job.as_ref().desc.as_ptr(),
                self.job.as_ref().ch.as_ref().unwrap().as_ptr(),
                Some(Job::io_completion),
                self as *const _ as *mut _,
            ) == 0
            {
                self.job.as_mut().n_inflight += 1;
            } else {
                eprintln!(
                    "failed to submit reset IO to {}",
                    self.job.as_ref().bdev.name()
                );
            }
        }
    }

    pub fn abort(&mut self) {
        extern "C" fn abort_done(
            bdev_io: *mut spdk_sys::spdk_bdev_io,
            success: bool,
            arg: *mut std::ffi::c_void,
        ) {
            dbg!(success);
            unsafe { spdk_bdev_free_io(bdev_io) };
        }

        unsafe {
            if spdk_bdev_abort(
                self.job.as_ref().desc.as_ptr(),
                self.job.as_ref().ch.as_ref().unwrap().as_ptr(),
                self as *const _ as *mut _,
                Some(abort_done),
                self as *const _ as *mut _,
            ) == 0
            {
                self.job.as_mut().n_inflight += 1;
            } else {
                eprintln!(
                    "failed to submit abort IO to {}",
                    self.job.as_ref().bdev.name()
                );
            }
        }
    }
}

#[derive(Debug)]
pub struct Job {
    /// that drives IO to a bdev using its own channel.
    bdev: Bdev,
    /// descriptor to the bdev
    desc: Descriptor,
    /// io channel used to submit IO
    ch: Option<IoChannel>,
    /// queue depth configured for this job
    qd: u64,
    /// io_size the io_size is the number of blocks submit per IO
    io_size: u64,
    /// blk_size of the underlying device
    blk_size: u32,
    /// num_blocks the device has
    num_blocks: u64,
    /// aligned set of IOs we can do
    io_blocks: u64,
    /// io queue
    queue: Vec<Io>,
    /// number of IO's completed
    n_io: u64,
    /// number of IO's currently inflight
    n_inflight: u32,
    /// generate random number between 0 and num_block
    //    rng: rand::rngs::ThreadRng,
    /// drain the job which means that we wait for all pending IO to complete
    /// and stop the run
    drain: bool,
    stopped: bool,
    /// number of seconds we are running
    period: u64,
    ///
    request_reset: bool,
    request_abort: bool,
}

impl Job {
    extern "C" fn io_completion(
        bdev_io: *mut spdk_sys::spdk_bdev_io,
        success: bool,
        arg: *mut std::ffi::c_void,
    ) {
        let ioq: &mut Io = unsafe { &mut *arg.cast() };
        let job = unsafe { ioq.job.as_mut() };

        // if !success {
        //     let bio = Bio::from(bdev_io);
        //     eprintln!("{:#?}", bio);
        // }

        job.n_io += 1;
        job.n_inflight -= 1;

        unsafe { spdk_bdev_free_io(bdev_io) }

        if job.n_inflight == 0 {
            job.stopped = true;
            unsafe { Box::from_raw(ioq.job.as_ptr()) };
            return;
        }

        if job.drain {
            return;
        }

        //let offset = (job.rng.gen::<u64>() % job.io_size) * job.io_blocks;
        ioq.next(0);
    }

    pub fn stop(&mut self) {
        self.drain = true;
    }

    fn as_ptr(&self) -> *mut Job {
        self as *const _ as *mut _
    }
    /// start the job that will dispatch an IO up to the provided queue depth
    fn start(mut self) -> NonNull<Job> {
        self.ch = self.desc.get_channel();
        let mut boxed = Box::new(self);
        let ptr = boxed.as_ptr();
        boxed.queue.iter_mut().for_each(|q| q.run(ptr));
        unsafe { NonNull::new_unchecked(Box::into_raw(boxed)) }
    }
}

#[derive(Default)]
pub struct Builder {
    /// bdev URI to create
    uri: String,
    /// queue depth
    qd: u64,
    /// size of each IO
    io_size: u64,
    /// type of workload to generate
    iot: IoType,
    bdev: Option<Bdev>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn uri(mut self, uri: &str) -> Self {
        self.uri = String::from(uri);
        self
    }

    pub fn qd(mut self, qd: u64) -> Self {
        self.qd = qd;
        self
    }

    pub fn io_size(mut self, io_size: u64) -> Self {
        self.io_size = io_size;
        self
    }

    pub fn rw(mut self, iot: IoType) -> Self {
        self.iot = iot;
        self
    }

    pub fn bdev(mut self, bdev: Bdev) -> Self {
        self.bdev = Some(bdev);
        self
    }

    pub async fn build(mut self) -> Job {
        let bdev = if self.bdev.is_some() {
            self.bdev.take().unwrap()
        } else {
            let name = bdev_create(&self.uri).await.unwrap();
            Bdev::lookup_by_name(&name).unwrap()
        };

        let desc = bdev.open(true).unwrap();

        let blk_size = bdev.block_len();
        let num_blocks = bdev.num_blocks();

        let io_size = self.io_size / blk_size as u64;
        let io_blocks = num_blocks / io_size;

        let mut queue = Vec::new();

        (0 .. self.qd).for_each(|offset| {
            queue.push(Io {
                buf: DmaBuf::new(self.io_size as u64, bdev.alignment())
                    .unwrap(),
                iot: IoType::READ,
                offset,
                job: NonNull::dangling(),
            });
        });

        Job {
            bdev,
            desc,
            ch: None,
            qd: self.qd,
            io_size,
            blk_size,
            num_blocks,
            queue,
            io_blocks,
            n_io: 0,
            n_inflight: 0,
            //rng: Default::default(),
            drain: false,
            period: 0,
            stopped: false,
            request_reset: false,
            request_abort: false,
        }
    }
}

pub struct JobQueue {
    inner: Mutex<Vec<NonNull<Job>>>,
}

impl Default for JobQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl JobQueue {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Vec::new()),
        }
    }

    pub fn start(&self, job: Job) {
        self.inner.lock().unwrap().push(job.start());
    }

    pub fn stop(&self, bdevname: &str) -> bool {
        let mut drained = false;
        self.inner.lock().unwrap().iter_mut().for_each(|j| {
            if unsafe { j.as_ref().bdev.name() } == bdevname {
                unsafe { j.as_mut().drain = true };
                drained = true;
            }
        });

        drained
    }

    pub fn send_reset(&self, bdevname: &str) {
        self.inner.lock().unwrap().iter_mut().for_each(|j| {
            let job = unsafe { j.as_mut() };
            job.request_reset = true;
        });
    }

    pub fn send_abort(&self, bdevname: &str) {
        self.inner.lock().unwrap().iter_mut().for_each(|j| {
            let job = unsafe { j.as_mut() };
            job.request_abort = true;
        });
    }
}
