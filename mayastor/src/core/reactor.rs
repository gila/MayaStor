use std::{
    borrow::{Borrow},
    ffi::CString,
    os::raw::c_void,
    pin::Pin,
    slice::Iter,
};

use crossbeam::channel::{unbounded, Receiver, Sender};
use futures::{Future};
use log::info;
use once_cell::sync::OnceCell;

use spdk_sys::{
    spdk_env_thread_launch_pinned,
    spdk_ring,
    spdk_thread_create,
    spdk_thread_lib_init,
    spdk_thread_send_msg,
};

use crate::{
    core::{
        Mthread,
        Cores,
    },
};

#[derive(Debug)]
pub struct Reactors(pub Vec<Reactor>);

// XXX likely its a good idea to not bindgen these and spell them out ourselves
// to avoid the ducktyping
#[derive(Debug)]
pub struct Ring(*mut spdk_ring);

unsafe impl Sync for Ring {}

unsafe impl Sync for Reactors {}

unsafe impl Send for Reactors {}

pub static REACTOR_LIST: OnceCell<Reactors> = OnceCell::new();

#[repr(C, align(64))]
#[derive(Debug)]
pub struct Reactor {
    threads: Vec<Mthread>,
    lcore: u32,
    flags: u32,
    sx: Sender<Pin<Box<dyn Future<Output = ()> + 'static>>>,
    rx: Receiver<Pin<Box<dyn Future<Output = ()> + 'static>>>,
}

type Task = async_task::Task<()>;

thread_local! {
    static QUEUE: (Sender<Task>, Receiver<Task>) = unbounded();
}

impl Reactors {
    pub fn init() {
        let rc = unsafe { spdk_thread_lib_init(None, 0) };
        assert_eq!(rc, 0);

        let reactors = Cores::count()
            .into_iter()
            .map(|c| {
                info!("init core: {}", c);
                Reactor::new(c)
            })
            .collect::<Vec<_>>();

        REACTOR_LIST.set(Reactors(reactors)).unwrap();
    }

    pub fn start() {
        Cores::count().into_iter().skip(1).for_each(|c| {
            let rc = unsafe {
                spdk_env_thread_launch_pinned(
                    c,
                    Some(Reactor::poll),
                    c as *const u32 as *mut c_void,
                )
            };
            assert_eq!(rc, 0)
        });
    }

    pub fn get(core: u32) -> Option<&'static Reactor> {
        Some(REACTOR_LIST.get().unwrap().0[core as usize].borrow())
    }

    pub fn iter() -> Iter<'static, Reactor> {
        REACTOR_LIST.get().unwrap().into_iter()
    }
}

impl<'a> IntoIterator for &'a Reactors {
    type Item = &'a Reactor;
    type IntoIter = ::std::slice::Iter<'a, Reactor>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl Reactor {
    fn new(core: u32) -> Self {
        let name = CString::new(format!("core_{}", core)).unwrap();
        let thread = Mthread(unsafe {
            spdk_thread_create(name.as_ptr(), std::ptr::null_mut())
        });

        let (sx, rx) =
            unbounded::<Pin<Box<dyn Future<Output = ()> + 'static>>>();
        Self {
            threads: vec![thread],
            lcore: core,
            flags: 0,
            sx,
            rx,
        }
    }
    extern "C" fn poll(core: *mut c_void) -> i32 {
       // let mut events: [*mut c_void; 8] = [std::ptr::null_mut(); 8];
        info!("polling reactor {}", core as u32);
        let reactor = Reactors::get(core as u32).unwrap();
        loop {
            reactor.poll_reactor();
            reactor.receive_futures();
        }
    }

    /// run the futures
    fn run_futures(&self) {
        QUEUE.with(|(_, r)| r.try_iter().for_each(|f| f.run()))
    }

    /// receive futures and spawn them
    fn receive_futures(&self) {
        let m: Vec<_> = self.rx.try_iter().collect();
        m.into_iter().for_each(|m| {
            self.spawn(m);
        });
    }

    /// send messages to the core/thread -- similar as spdk_thread_send_msg()
    pub fn send_future<F>(&self, future: F)
    where
        F: Future<Output = ()> + 'static,
    {
        self.sx.send(Box::pin(future)).unwrap();
    }

    pub fn spawn<F, R>(&self, future: F) -> async_task::JoinHandle<R, ()>
    where
        F: Future<Output = R> + 'static,
        R: 'static,
    {
        let schedule = |t| QUEUE.with(|(s, _)| s.send(t).unwrap());
        let (task, handle) = async_task::spawn_local(future, schedule, ());
        task.schedule();
        handle
    }

    /// in effect the same as send_message except these use the rte_ring subsystem. It should be
    /// faster compared to the send_message() counter part as it has pre allocated structures.
    pub fn spawn_on<F: 'static>(&self, f: F)
    where
        F: Future<Output = ()>,
    {
        extern "C" fn unwrap<F: 'static>(args: *mut c_void)
        where
            F: Future<Output = ()>,
        {
            let f: Box<F> = unsafe { Box::from_raw(args as *mut F) };
            let schedule = |t| QUEUE.with(|(s, _)| s.send(t).unwrap());
            let (task, _handle) = async_task::spawn_local(*f, schedule, ());
            task.schedule();
        }

        let ptr = Box::into_raw(Box::new(f)) as *mut c_void;
        let e = unsafe {
            spdk_thread_send_msg(self.threads[0].0, Some(unwrap::<F>), ptr)
        };
        if e != 0 {
            error!("failed to dispatch future to mempool");
            unsafe { Box::from_raw(ptr); }
        }
    }

    /// poll this reactor to complete any work that is pending
    pub fn poll_reactor(&self) {

        self.threads[0].with(|| {
            self.run_futures();
        });

        // if there are any other threads poll them now skipping thread 0 as it has been
        // polled already running the futures
        self.threads.iter().skip(1).for_each(|t| {
            t.poll();
        });
    }
}

impl Drop for Reactor {
    fn drop(&mut self) {
    }
}

