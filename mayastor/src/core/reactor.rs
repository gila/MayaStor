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
use std::cell::{Ref, RefCell};

#[derive(Debug)]
pub struct ReactorList(Vec<Reactor>);

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

unsafe impl Sync for ReactorList {}
unsafe impl Send for ReactorList {}

pub static REACTOR_LIST: OnceCell<ReactorList> = OnceCell::new();
pub static MEM_POOL: OnceCell<MemPool> = OnceCell::new();

#[repr(C, align(64))]
#[derive(Debug)]
pub struct Reactor {
    threads: Vec<Mthread>,
    lcore: u32,
    flags: u32,
    events: Ring,
    pool: RefCell<LocalPool>,
}

pub fn get_reactor(core: u32) -> Option<&'static Reactor> {
    REACTOR_LIST.get().unwrap().0.get(core as usize)
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
            pool: RefCell::new(LocalPool::new()),
        }
    }

    pub fn run_futures(&self) {
        self.pool.borrow_mut().run_until_stalled();
    }
}

impl Drop for Reactor {
    fn drop(&mut self) {
        unsafe { spdk_ring_free(*self.events) }
    }
}

pub unsafe fn reactors_init() {
    // first we need to allocate a mempool that holds a pool of events such that
    // we can grab these events from the pool as fast as we can.
    let pool_name = format!("evtpool_{}", libc::getpid());
    let pool_name = CString::new(pool_name).unwrap();

    let _ = MEM_POOL.get_or_init(|| {
        let mem_pool = spdk_mempool_create(
            pool_name.into_raw(),
            262144 - 1,
            std::mem::size_of::<spdk_event>(),
            SPDK_MEMPOOL_DEFAULT_CACHE_SIZE as usize,
            SPDK_ENV_SOCKET_ID_ANY,
        );

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

    REACTOR_LIST.set(ReactorList(reactors)).unwrap();

    // dbg!(REACTOR_LIST.get());
}

pub fn get_event() -> &'static mut cb_event {
    let e = unsafe { spdk_mempool_get(MEM_POOL.get().unwrap().0) };
    unsafe { &mut *(e as *const _ as *mut cb_event) }
}

pub struct cb_event {
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

pub extern "C" fn reactor_poll(core: *mut c_void) -> i32 {
    // install executor

    let core = core as u32;

    dbg!("polling core {}", core);

    let mut events: [*mut c_void; 8] = [std::ptr::null_mut(); 8];

    loop {
        let reactor = REACTOR_LIST.get().unwrap().0[core as usize].borrow();

        let count =
            unsafe { spdk_ring_dequeue(**&reactor.events, &mut events[0], 8) };

        if count == 0 {
            // adaptive polling
            info!("no events in this reactor");
            std::thread::sleep(Duration::from_secs(2));
        }

        reactor.threads[0].with(|| {
            events.iter().take(count).for_each(|e| {
                let event = unsafe { &mut *(e as *const _ as *mut cb_event) };
                (event.func)(event.arg1, event.arg2);
            });
            reactor.run_futures();
        });

        unsafe {
            spdk_mempool_put_bulk(MEM_POOL.get().unwrap().0, &mut events[0], 8)
        }

        reactor.threads.iter().skip(1).for_each(|t| unsafe {
            spdk_thread_poll(t.0, 0, 0);
        });
    }
    0
}

async fn my_async() {
    println!("im a future running on core {}", unsafe {
        spdk_env_get_current_core()
    });
}

pub fn reactors_start() {
    Cores::count().into_iter().skip(1).for_each(|c| {
        let rc = unsafe {
            spdk_env_thread_launch_pinned(
                c,
                Some(reactor_poll),
                c as *const u32 as *mut c_void,
            )
        };

        assert_eq!(rc, 0)
    });

    extern "C" fn thread_fn(arg: *mut c_void) {
        println!("Hello from core {}", unsafe { spdk_env_get_current_core() });
    }

    REACTOR_LIST.get().unwrap().0.iter().for_each(|r| {
        r.threads.iter().for_each(|t| unsafe {
            spdk_thread_send_msg(t.0, Some(thread_fn), std::ptr::null_mut());
        });
    });

    REACTOR_LIST.get().unwrap().0.iter().for_each(|r| {
        let p = r.pool.borrow_mut();
        p.spawner()
            .spawn_local(async {
                my_async().await;
            })
            .unwrap();
    });

    REACTOR_LIST
        .get()
        .unwrap()
        .0
        .iter()
        .filter(|r| r.lcore == 3)
        .for_each(|r| {
            let p = r.pool.borrow_mut();
            p.spawner()
                .spawn_local(async {
                    async {
                        unsafe {
                            spdk_subsystem_init(None, std::ptr::null_mut());
                            spdk_rpc_initialize(
                                "/var/tmp/spdk.sock\0".as_ptr() as *mut _,
                            );
                            spdk_rpc_set_state(SPDK_RPC_RUNTIME);
                        }
                        dbg!(target::nvmf::init("127.0.0.1".into())
                            .await
                            .unwrap());
                    }
                    .await;
                })
                .unwrap();
        });
}
