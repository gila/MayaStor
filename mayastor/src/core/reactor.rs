use std::{
    borrow::{Borrow, BorrowMut},
    ffi::CString,
    ops::{Deref, DerefMut},
    os::raw::c_void,
    time::Duration,
};

use futures::{
    executor::{LocalPool, LocalSpawner},
    task::LocalSpawnExt,
    Future,
};
use log::info;
use once_cell::sync::OnceCell;

use spdk_sys::{
    spdk_env_get_current_core,
    spdk_env_thread_launch_pinned,
    spdk_event,
    spdk_mempool,
    spdk_mempool_create,
    spdk_mempool_get,
    spdk_mempool_put_bulk,
    spdk_ring,
    spdk_ring_create,
    spdk_ring_dequeue,
    spdk_ring_enqueue,
    spdk_ring_free,
    spdk_rpc_set_state,
    spdk_thread_create,
    spdk_thread_lib_init,
    spdk_thread_poll,
    spdk_thread_send_msg,
    SPDK_ENV_SOCKET_ID_ANY,
    SPDK_MEMPOOL_DEFAULT_CACHE_SIZE,
    SPDK_RING_TYPE_MP_SC,
    SPDK_RPC_RUNTIME,
};

use crate::{
    core::{
        env::{spdk_rpc_initialize, spdk_subsystem_init},
        event::Mthread,
        Cores,
    },
    target,
};
use std::{
    cell::{RefCell, UnsafeCell},
    slice::Iter,
};

#[derive(Debug)]
pub struct Reactors(pub Vec<Reactor>);

// XXX likely its a good idea to not bindgen these and spell them out ourselves
// to avoid the ducktyping
#[derive(Debug)]
pub struct Ring(*mut spdk_ring);

unsafe impl Sync for Ring {}
unsafe impl Send for Ring {}

impl Deref for Ring {
    type Target = *mut spdk_ring;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Ring {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for Ring {
    fn drop(&mut self) {
        unsafe { spdk_ring_free(self.0) }
    }
}

#[derive(Debug)]
pub struct MemPool(*mut spdk_mempool);

unsafe impl Sync for MemPool {}
unsafe impl Send for MemPool {}

unsafe impl Sync for Reactors {}
unsafe impl Send for Reactors {}

pub static REACTOR_LIST: OnceCell<Reactors> = OnceCell::new();
pub static MEM_POOL: OnceCell<MemPool> = OnceCell::new();

#[repr(C, align(64))]
#[derive(Debug)]
pub struct Reactor {
    threads: Vec<Mthread>,
    lcore: u32,
    flags: u32,
    events: Ring,
    pool: UnsafeCell<LocalPool>,
}

impl Reactors {
    pub fn init() {
        let pool_name = format!("evtpool_{}", unsafe { libc::getpid() });
        let pool_name = CString::new(pool_name).unwrap();

        let _ = MEM_POOL.get_or_init(|| {
            let mem_pool = unsafe {
                spdk_mempool_create(
                    pool_name.into_raw(),
                    262144 - 1,
                    std::mem::size_of::<spdk_event>(),
                    SPDK_MEMPOOL_DEFAULT_CACHE_SIZE as usize,
                    SPDK_ENV_SOCKET_ID_ANY,
                )
            };

            if mem_pool.is_null() {
                panic!("failed to allocate mem pool, cannot continue");
            }
            // dbg!(&mem_pool);
            MemPool(mem_pool)
        });

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

    pub fn spawn<F: Future<Output = ()> + 'static>(core: u32, f: F) {
        if let Some(c) = REACTOR_LIST
            .get()
            .unwrap()
            .into_iter()
            .find(|r| r.lcore == core)
        {
            unsafe { (*c.pool.get()).spawner().spawn_local(f).unwrap() };
        } else {
            panic!("no such core")
        }
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
        let r = unsafe {
            spdk_ring_create(
                SPDK_RING_TYPE_MP_SC,
                665536,
                SPDK_ENV_SOCKET_ID_ANY,
            )
        };

        if r.is_null() {
            panic!("failed to create for ring")
        }

        let name = CString::new(format!("core_{}", core)).unwrap();
        let thread = Mthread(unsafe {
            spdk_thread_create(name.as_ptr(), std::ptr::null_mut())
        });

        Self {
            threads: vec![thread],
            lcore: core,
            flags: 0,
            events: Ring(r),
            pool: UnsafeCell::new(LocalPool::new()),
        }
    }

    //    pub fn run_futures(&self) {
    //        self.pool.borrow_mut().run_until_stalled();
    //    }

    extern "C" fn poll(core: *mut c_void) -> i32 {
        if let Some(r) = REACTOR_LIST
            .get()
            .unwrap()
            .into_iter()
            .find(|r| r.lcore == core as u32)
        {
            let mut events: [*mut c_void; 8] = [std::ptr::null_mut(); 8];
            loop {
                r.poll_once(&mut events);
            }
        }
        0
    }

    //    fn poll_once(core: u32) -> usize {
    //        let core = core as u32;
    //
    //        dbg!("polling core {}", core);
    //
    //        let mut work = 0;
    //
    //        let mut events: [*mut c_void; 8] = [std::ptr::null_mut(); 8];
    //        let reactor = REACTOR_LIST.get().unwrap().0[core as
    // usize].borrow();
    //
    //        let count =
    //            unsafe { spdk_ring_dequeue(**&reactor.events, &mut events[0],
    // 8) };
    //
    //        if count != 0 {
    //            // adaptive polling
    //            info!("events in this reactor");
    //            std::thread::sleep(Duration::from_secs(2));
    //
    //            reactor.threads[0].with(|| {
    //                events.iter().take(count).for_each(|e| {
    //                    let event =
    //                        unsafe { &mut *(e as *const _ as *mut cb_event) };
    //                    (event.func)(event.arg1, event.arg2);
    //                });
    //                reactor.run_futures();
    //            });
    //
    //            unsafe {
    //                spdk_mempool_put_bulk(
    //                    MEM_POOL.get().unwrap().0,
    //                    &mut events[0],
    //                    8,
    //                )
    //            }
    //
    //            work = 1
    //        }
    //
    //        // if there was work in the main reactor message pool, we have
    // already        // polled thread 0 as we need its context, there for
    // skip it.        reactor.threads.iter().skip(work).for_each(|t| {
    //            work += t.poll();
    //        });
    //
    //        work
    //    }

    pub fn poll_once(&self, events: &mut [*mut c_void]) -> i32 {
        let mut work = 0;
        let count =
            unsafe { spdk_ring_dequeue(*self.events, &mut events[0], 8) };

        self.threads[0].with(|| {
            if count == 0 {
                events.iter().take(count).for_each(|e| {
                    let event = unsafe { &mut *(e as *const _ as *mut Event) };
                    (event.func)(event.arg1, event.arg2);
                });

                unsafe {
                    spdk_mempool_put_bulk(
                        MEM_POOL.get().unwrap().0,
                        &mut events[0],
                        8,
                    )
                }
            }

            unsafe { (*self.pool.get()).run_until_stalled() };
            work = 1;
        });

        self.threads.iter().skip(work).for_each(|t| {
            work += t.poll();
        });

        work as i32
    }
}

impl Drop for Reactor {
    fn drop(&mut self) {
        unsafe { spdk_ring_free(*self.events) }
    }
}

pub fn get_event() -> &'static mut Event {
    let e = unsafe { spdk_mempool_get(MEM_POOL.get().unwrap().0) };
    unsafe { &mut *(e as *const _ as *mut Event) }
}

pub struct Event {
    lcore: u32,
    func: extern "C" fn(*mut c_void, *mut c_void),
    arg1: *mut c_void,
    arg2: *mut c_void,
}

extern "C" fn hello_jan(arg1: *mut c_void, arg2: *mut c_void) {
    println!("Hello Jan from core {}", unsafe {
        spdk_env_get_current_core()
    });
}

async fn my_async() {
    println!("im a future running on core {}", unsafe {
        spdk_env_get_current_core()
    });
}

pub fn reactors_start() {
    extern "C" fn thread_fn(arg: *mut c_void) {
        println!("Hello from core {}", unsafe { spdk_env_get_current_core() });
    }

    REACTOR_LIST.get().unwrap().0.iter().for_each(|r| {
        r.threads.iter().for_each(|t| unsafe {
            spdk_thread_send_msg(t.0, Some(thread_fn), std::ptr::null_mut());
        });
    });

    //    REACTOR_LIST.get().unwrap().0.iter().for_each(|r| {
    //        let p = r.pool.borrow_mut();
    //        p.spawner()
    //            .spawn_local(async {
    //                my_async().await;
    //            })
    //            .unwrap();
    //    });

    let _r = Reactors::spawn(3, async {
        async {
            unsafe {
                spdk_subsystem_init(None, std::ptr::null_mut());
                spdk_rpc_initialize("/var/tmp/spdk.sock\0".as_ptr() as *mut _);
                spdk_rpc_set_state(SPDK_RPC_RUNTIME);
            }
            dbg!(target::nvmf::init("127.0.0.1".into()).await.unwrap());
        }
        .await;
    });
}
