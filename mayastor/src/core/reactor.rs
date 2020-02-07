use crossbeam::channel::{unbounded, Receiver, Sender};
use futures::Future;
use log::info;
use std::{
    cell::RefCell,
    os::raw::c_void,
    pin::Pin,
    slice::Iter,
    sync::atomic::Ordering,
};

use spdk_sys::{
    spdk_env_thread_launch_pinned,
    spdk_env_thread_wait_all,
    spdk_ring,
    spdk_set_thread,
    spdk_thread_create,
    spdk_thread_lib_init,
    spdk_thread_send_msg,
};

use crate::core::{Cores, Mthread};
use crossbeam::sync::Parker;
use futures::task::{Context, Poll, Waker};
use once_cell::sync::OnceCell;
use std::{cell::Cell, time::Duration};
pub(crate) const INIT: usize = 1 << 1;
pub(crate) const RUNNING: usize = 1 << 2;
pub(crate) const SHUTDOWN: usize = 1 << 3;
pub(crate) const SUSPEND: usize = 1 << 4;
pub(crate) const DEVELOPER_DELAY: usize = 1 << 5;

#[derive(Debug)]
pub struct Reactors(Vec<Reactor>);

unsafe impl Sync for Reactors {}
unsafe impl Send for Reactors {}

pub static REACTOR_LIST: OnceCell<Reactors> = OnceCell::new();
pub static mut MASTER_CORE_STOPPED: bool = false;
#[repr(C, align(64))]
#[derive(Debug)]
pub struct Reactor {
    threads: Vec<Mthread>,
    lcore: u32,
    flags: Cell<usize>,
    sx: Sender<Pin<Box<dyn Future<Output = ()> + 'static>>>,
    rx: Receiver<Pin<Box<dyn Future<Output = ()> + 'static>>>,
}

type Task = async_task::Task<()>;

thread_local! {
    /// This queue holds any in coming futures from other cores
    static QUEUE: (Sender<Task>, Receiver<Task>) = unbounded();
}

impl Reactors {
    /// initialize the reactor subsystem for each core assigned to us
    pub fn init() {
        REACTOR_LIST.get_or_init(|| {
            let rc = unsafe { spdk_thread_lib_init(None, 0) };
            assert_eq!(rc, 0);

            Reactors(
                Cores::count()
                    .into_iter()
                    .map(|c| {
                        info!("init core: {}", c);
                        Reactor::new(c)
                    })
                    .collect::<Vec<_>>(),
            )
        });
    }

    /// launch the poll loop on the master core, this is implemented somewhat
    /// different from the remote cores.
    pub fn launch_master() {
        assert_eq!(Cores::current(), Cores::first());
        Reactor::poll(Cores::current() as *const u32 as *mut c_void);
        // wait for all other cores to exit
        unsafe { spdk_env_thread_wait_all() };
    }

    /// start polling the reactors on the given core, when multiple cores are
    /// involved they must be running during init to process any incomming
    /// messages
    pub fn launch_remote(core: u32) -> Result<(), ()> {
        // the master core -- who is the only core that can call this function
        // should not be launched this way. For that use
        // ['launch_master`]
        if core == Cores::current() {
            return Ok(());
        }

        if Cores::count().into_iter().any(|c| c == core) {
            let rc = unsafe {
                spdk_env_thread_launch_pinned(
                    core,
                    Some(Reactor::poll),
                    core as *const u32 as *mut c_void,
                )
            };
            if rc == 0 {
                return Ok(());
            }
        }

        error!("failed to launch core {}", core);
        Err(())
    }

    /// get a reference to a ['Reactor'] associated with the given core.
    pub fn get_by_core(core: u32) -> Option<&'static Reactor> {
        Reactors::iter().find(|c| c.lcore == core)
    }

    /// get a reference to a reactor on the current core
    pub fn current() -> Option<&'static Reactor> {
        Self::get_by_core(Cores::current())
    }

    /// returns an iterator over all reactors
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
    /// create a new ['Reactor'] instance
    fn new(core: u32) -> Self {
        // allocate a new thread which provides the SPDK context
        let t = Mthread::new(format!("core_{}", core));
        // create a channel to receive futures on
        let (sx, rx) =
            unbounded::<Pin<Box<dyn Future<Output = ()> + 'static>>>();

        Self {
            threads: vec![t],
            lcore: core,
            flags: Cell::new(INIT),
            sx,
            rx,
        }
    }

    /// this function gets called by DPDK
    extern "C" fn poll(core: *mut c_void) -> i32 {
        debug!("Start polling of reactor {}", core as u32);
        let reactor = Reactors::get_by_core(core as u32).unwrap();
        if cfg!(debug_assertions) {
            reactor.developer_delayed();
        } else {
            reactor.running();
        }
        // loops
        reactor.poll_reactor();
        0
    }

    /// run the futures received on the channel
    fn run_futures(&self) {
        QUEUE.with(|(_, r)| r.try_iter().for_each(|f| f.run()))
    }

    /// receive futures if any, and drive to completion
    fn receive_futures(&self) {
        self.rx.try_iter().for_each(|m| {
            self.spawn_local(m);
        });
    }

    /// send messages to the core/thread -- similar as spdk_thread_send_msg()
    pub fn send_future<F>(&self, future: F)
    where
        F: Future<Output = ()> + 'static,
    {
        self.sx.send(Box::pin(future)).unwrap();
    }

    /// spawn a future locally on this core
    fn spawn_local<F, R>(&self, future: F) -> async_task::JoinHandle<R, ()>
    where
        F: Future<Output = R> + 'static,
        R: 'static,
    {
        let schedule = |t| QUEUE.with(|(s, _)| s.send(t).unwrap());
        let (task, handle) = async_task::spawn_local(future, schedule, ());
        task.schedule();
        handle
    }

    /// spawn a future locally on the current core block until the future is
    /// completed
    pub fn block_on<F, R>(future: F) -> Option<R>
    where
        F: Future<Output = R> + 'static,
        R: 'static,
    {
        let reactor = Reactors::current().unwrap();
        //reactor.thread_enter();
        let schedule = |t| QUEUE.with(|(s, _)| s.send(t).unwrap());
        let (task, handle) = async_task::spawn_local(future, schedule, ());

        let cx = handle.waker();
        let cx = &mut Context::from_waker(&cx);

        pin_utils::pin_mut!(handle);
        task.schedule();
        //reactor.thread_exit();
        loop {
            match handle.as_mut().poll(cx) {
                Poll::Ready(output) => {
                    //reactor.thread_exit();
                    return output;
                }
                Poll::Pending => {
                    reactor.receive_futures();
                    reactor.threads[0].with(|| {
                        reactor.run_futures();
                    });
                }
            }
        }
    }

    /// set the state of this reactor
    fn set_state(&self, state: usize) {
        match state {
            SUSPEND | RUNNING | SHUTDOWN | DEVELOPER_DELAY => {
                self.flags.set(state)
            }
            _ => panic!("Invalid state"),
        }
    }

    /// suspend (sleep in a loop) the reactor until the state has been set to
    /// running again
    pub fn suspend(&self) {
        self.set_state(SUSPEND)
    }

    /// set the state of the reactor to running. In this state the reactor will
    /// poll for work on the thread message pools as well as its own queue
    /// to launch futures.
    pub fn running(&self) {
        self.set_state(RUNNING)
    }

    pub fn developer_delayed(&self) {
        warn!("core {} set to developer delayed poll mode", self.lcore);
        self.set_state(DEVELOPER_DELAY)
    }

    pub fn shutdown(&self) {
        warn!("shutdown requested for core {}", self.lcore);
        self.set_state(SHUTDOWN);
    }
    /// returns the current state of the reactor
    pub fn get_sate(&self) -> usize {
        self.flags.get()
    }

    /// returns core number of this reactor
    pub fn core(&self) -> u32 {
        self.lcore
    }

    /// poll this reactor to complete any work that is pending
    pub fn poll_reactor(&self) {
        loop {
            match self.flags.get() {
                SUSPEND => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                RUNNING => {
                    self.poll_once();
                }
                SHUTDOWN => {
                    info!("reactor {} shutdown requested", self.lcore);
                    break;
                }
                DEVELOPER_DELAY => {
                    std::thread::sleep(Duration::from_millis(1));
                    self.poll_once();
                }
                _ => panic!("invalid reactor state {}", self.flags.get()),
            }
        }

        debug!("initiating shutdown");
        // clean up the threads
        self.threads.iter().for_each(|t| t.destroy());

        if self.lcore == Cores::first() {
            unsafe {
                MASTER_CORE_STOPPED = true;
            }
        }

        debug!("poll loop exit")
    }

    /// set the proper SPDK context before executing
    fn thread_enter(&self) {
        self.threads[0].enter();
    }

    /// unset the above context
    fn thread_exit(&self) {
        self.threads[0].exit();
    }

    /// execute f within a thread context
    pub fn on_thread<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        self.thread_enter();
        let r = f();
        self.thread_exit();
        r
    }

    /// polls the reactor only once for any work regardless of its state
    pub fn poll_once(&self) {
        // assert_eq!(unsafe {spdk_sys::spdk_get_thread()},
        // std::ptr::null_mut());
        self.receive_futures();
        self.threads[0].with(|| {
            self.run_futures();
        });

        // if there are any other threads poll them now skipping thread 0 as it
        // has been polled already running the futures
        self.threads.iter().skip(1).for_each(|t| {
            t.enter().poll().exit();
        });
    }
}
impl Drop for Reactor {
    fn drop(&mut self) {
        info!("dropping {:?}", self);
    }
}
