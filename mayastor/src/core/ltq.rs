//!
//! Local Task Queue is the queue that holds runnable futures
use std::{
    cell::RefCell,
    collections::VecDeque,
    fmt,
    future::Future,
    marker::PhantomData,
    mem::ManuallyDrop,
    panic::{RefUnwindSafe, UnwindSafe},
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
    thread,
    thread::ThreadId,
};

use async_task::{Runnable, Task};

use crate::core::{Cores, Mthread, Reactors};

#[inline]
fn thread_id() -> ThreadId {
    thread_local! { static ID: ThreadId = thread::current().id(); }
    ID.try_with(|id| *id)
        .unwrap_or_else(|_| thread::current().id())
}

#[derive(Debug)]
pub struct LocalTaskQueue {
    queue: RefCell<VecDeque<Runnable>>,
}

impl LocalTaskQueue {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            queue: RefCell::new(VecDeque::new()),
        })
    }

    fn push(&self, r: Runnable) {
        self.queue.borrow_mut().push_back(r)
    }

    fn pop_front(&self) -> Option<Runnable> {
        self.queue.borrow_mut().pop_front()
    }
}

#[derive(Debug)]
struct Checked<F> {
    id: ThreadId,
    mthread: Option<Mthread>,
    core: u32,
    inner: ManuallyDrop<F>,
}

impl<F> Drop for Checked<F> {
    fn drop(&mut self) {
        assert_eq!(
            self.id,
            thread_id(),
            "local task dropped by a thread that didn't spawn it"
        );
        assert_eq!(
            self.core,
            Cores::current(),
            "local task dropped on different core then spawned"
        );
        assert_eq!(
            self.mthread,
            Mthread::current(),
            "task dropped on different thread that spawned it"
        );
        unsafe {
            ManuallyDrop::drop(&mut self.inner);
        }
    }
}

impl<F: Future> Future for Checked<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        assert_eq!(
            self.id,
            thread_id(),
            "local task polled by a thread that didn't spawn it"
        );

        assert_eq!(
            self.mthread,
            Mthread::current(),
            "task dropped on different thread that spawned it"
        );
        unsafe { self.map_unchecked_mut(|c| &mut *c.inner).poll(cx) }
    }
}

#[derive(Debug)]
pub struct LocalExecutor {
    local_queue: Rc<LocalTaskQueue>,
    callback: Callback,
    _marker: PhantomData<*mut std::os::raw::c_void>,
}

impl LocalExecutor {
    pub fn loop_on<F, R>(&self, future: F) -> Option<R>
    where
        F: Future<Output = R> + 'static,
        R: 'static,
    {
        // Wrap the future into one that checks which thread it's on.
        let future = Checked {
            id: thread_id(),
            mthread: Mthread::current(),
            core: Cores::current(),
            inner: ManuallyDrop::new(future),
        };

        // The function that schedules a runnable task when it gets woken up.
        let schedule = move |runnable: Runnable| {
            let queue = Rc::downgrade(&self.local_queue);
            queue.upgrade().unwrap().push(runnable);
            self.callback.call();
        };

        let (runnable, task) =
            unsafe { async_task::spawn_unchecked(future, schedule) };

        let waker = runnable.waker();
        let cx = &mut Context::from_waker(&waker);
        pin_utils::pin_mut!(task);
        runnable.schedule();
        let reactor = Reactors::master();

        loop {
            match task.as_mut().poll(cx) {
                Poll::Ready(output) => {
                    return Some(output);
                }
                Poll::Pending => {
                    reactor.poll_once();
                }
            };
        }
    }

    pub fn spawn_local<F>(&self, future: F) -> Task<F::Output>
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        // Wrap the future into one that checks which thread it's on.
        let future = Checked {
            id: thread_id(),
            mthread: Mthread::current(),
            core: Cores::current(),
            inner: ManuallyDrop::new(future),
        };

        // The function that schedules a runnable task when it gets woken up.
        let schedule = move |runnable: Runnable| {
            let queue = Rc::downgrade(&self.local_queue);
            queue.upgrade().unwrap().push(runnable);
            self.callback.call();
        };

        let (runnable, task) =
            unsafe { async_task::spawn_unchecked(future, schedule) };
        runnable.run();
        task
    }

    pub fn new() -> Self {
        Self {
            local_queue: LocalTaskQueue::new(),
            callback: Callback::new(|| {
                info!("future scheduled");
            }),
            _marker: Default::default(),
        }
    }

    pub fn empty(&self) -> bool {
        !self.local_queue.queue.borrow().is_empty()
    }

    pub fn run_till_stalled(&self) {
        while let Some(r) = self.local_queue.pop_front() {
            r.run();
        }
        // self.local_queue
        //     .queue
        //     .borrow_mut()
        //     .iter_mut()
        //     .for_each(|f| {
        //         let _ = f.run();
        //     })
    }
}

impl UnwindSafe for LocalExecutor {}
impl RefUnwindSafe for LocalExecutor {}
/// A cloneable callback function.
#[derive(Clone)]
struct Callback(Rc<dyn Fn()>);

impl Callback {
    fn new(f: impl Fn() + 'static) -> Callback {
        Callback(Rc::new(f))
    }

    fn call(&self) {
        (self.0)();
    }
}

impl fmt::Debug for Callback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("<callback>").finish()
    }
}
