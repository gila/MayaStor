use crate::IoType;
use mayastor::{
    core::{mayastor_env_stop, Bdev, Descriptor, DmaBuf, IoChannel, Reactors},
    nexus_uri::bdev_create,
};

use once_cell::sync::OnceCell;
use rand::Rng;
use spdk_sys::{
    spdk_bdev_free_io,
    spdk_bdev_read,
    spdk_bdev_write,
    spdk_io_channel,
};

use std::{
    cell::RefCell,
    ptr::NonNull,
    sync::{LockResult, Mutex, MutexGuard},
};

pub struct JobList {
    inner: Mutex<Vec<Box<Job>>>,
    num: u32,
}

impl JobList {
    pub fn get() -> &'static JobList {
        static JOBLIST: OnceCell<JobList> = OnceCell::new();

        &JOBLIST.get_or_init(|| JobList {
            inner: Mutex::new(vec![]),
            num: 0,
        })
    }

    pub fn add(&self, job: Box<Job>) {
        self.inner.lock().unwrap().push(job);
    }

    pub fn drain(&self, bdev_name: String) {
        let mut list = self.inner.lock().unwrap();
        list.retain(|this| this.bdev_name != bdev_name);
    }

    pub fn drain_all(&self) {
        self.inner
            .lock()
            .unwrap()
            .iter_mut()
            .for_each(|j| j.drain = true);
    }

    pub fn empty(&self) -> bool {
        self.inner.lock().unwrap().is_empty()
    }

    pub fn stats(&self) -> i32 {
        let mut total_io_per_second = 0;
        let mut total_mb_per_second = 0;
        self.inner.lock().unwrap().iter_mut().for_each(|j| {
            j.period += 1;
            let io_per_second = j.n_io / j.period;
            let mb_per_second = io_per_second * j.io_size / (1024 * 1024);
            println!(
                "\r {:20}: {:10} IO/s {:10}: MB/s",
                j.name(),
                io_per_second,
                mb_per_second
            );
            total_io_per_second += io_per_second;
            total_mb_per_second += mb_per_second;
            println!(
                "\r {:20}: {:10} IO/s {:10}: MB/s\n",
                "Total", total_io_per_second, total_mb_per_second
            );
        });
        0
    }
}

/// a Job refers to a set of work typically defined by either time or size
/// that drives IO to a bdev using its own channel.
#[derive(Debug)]
pub(crate) struct Job {
    bdev_name: String,
    /// descriptor to the bdev
    desc: Descriptor,
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
    rng: rand::rngs::ThreadRng,
    /// drain the job which means that we wait for all pending IO to complete
    /// and stop the run
    drain: bool,
    /// number of seconds we are running
    period: u64,
}

impl Job {
    /// io completion callback
    extern "C" fn io_completion(
        bdev_io: *mut spdk_sys::spdk_bdev_io,
        success: bool,
        arg: *mut std::ffi::c_void,
    ) {
        let ioq: &mut Io = unsafe { &mut *arg.cast() };
        let job = unsafe { ioq.job.as_mut() };

        if !success {
            eprintln!("IO error for bdev {}, LBA {}", job.name(), ioq.offset);
        }

        job.n_io += 1;
        job.n_inflight -= 1;

        unsafe { spdk_bdev_free_io(bdev_io) }

        if job.drain && job.n_inflight == 0 {
            let jl = JobList::get();
            jl.drain(job.bdev_name.clone());
            if jl.empty() {
                Reactors::master().send_future(async {
                    mayastor_env_stop(0);
                });
            }
        }

        if job.drain {
            return;
        }

        let offset = (job.rng.gen::<u64>() % job.io_size) * job.io_blocks;
        ioq.next(offset);
    }

    /// construct a new job
    pub async fn new(bdev: &str, size: u64, qd: u64) -> Box<Self> {
        let bdev = bdev_create(&bdev)
            .await
            .map_err(|e| {
                eprintln!("Failed to open URI {}: {}", bdev, e.to_string());
                std::process::exit(1);
            })
            .map(|name| Bdev::lookup_by_name(&name).unwrap())
            .unwrap();

        let desc = bdev.open(true).unwrap();

        let blk_size = bdev.block_len();
        let num_blocks = bdev.num_blocks();

        let io_size = size / blk_size as u64;
        let io_blocks = num_blocks / io_size;

        let mut queue = Vec::new();

        (0 ..= qd).for_each(|offset| {
            queue.push(Io {
                buf: DmaBuf::new(size, bdev.alignment()).unwrap(),
                iot: IoType::READ,
                offset,
                job: NonNull::dangling(),
                ch: None,
            });
        });

        Box::new(Self {
            bdev_name: bdev.name(),
            desc,
            qd,
            io_size: size,
            blk_size,
            num_blocks,
            queue,
            io_blocks,
            n_io: 0,
            n_inflight: 0,
            rng: Default::default(),
            drain: false,
            period: 0,
        })
    }

    fn as_ptr(&self) -> *mut Job {
        self as *const _ as *mut _
    }

    pub fn name(&self) -> &str {
        &self.bdev_name
    }

    /// start the job that will dispatch an IO up to the provided queue depth
    pub fn run(mut self: Box<Self>) {
        let ptr = self.as_ptr();
        self.queue.iter_mut().for_each(|q| q.run(ptr));
        JobList::get().add(self)
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
    ch: Option<IoChannel>,
}

impl Io {
    /// start submitting
    fn run(&mut self, job: *mut Job) {
        let job = NonNull::<Job>::new(job).unwrap();
        self.ch = unsafe { job.as_ref().desc.get_channel() };

        match self.iot {
            IoType::READ => self.read(0),
            IoType::WRITE => self.write(0),
        };
    }

    /// dispatch the next IO, this is called from within the completion callback
    pub fn next(&mut self, offset: u64) {
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
                self.ch.as_ref().unwrap().as_ptr(),
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
                    self.job.as_ref().name()
                );
            }
        };
    }

    /// dispatch write IO at given offset
    fn write(&mut self, offset: u64) {
        unsafe {
            if spdk_bdev_write(
                self.job.as_ref().desc.as_ptr(),
                self.ch.as_ref().unwrap().as_ptr(),
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
                    self.job.as_ref().name()
                );
            }
        };
    }
}
