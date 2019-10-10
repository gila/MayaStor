use core::{fmt, pin::Pin};
use futures::{
    future::Future,
    task::{Context, Poll, Waker},
};
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

#[derive(Debug)]
enum State<T> {
    Open(Rc<RefCell<Inner<T>>>),
    Closed(Option<T>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Canceled;

impl fmt::Display for Canceled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "oneshot canceled")
    }
}

#[derive(Debug)]
pub struct Inner<T> {
    data: Option<T>,
    rx_task: Option<Waker>,
    tx_task: Option<Waker>,
}

impl<T> Inner<T> {
    fn new() -> Inner<T> {
        Inner {
            data: None,
            rx_task: None,
            tx_task: None,
        }
    }
}

#[derive(Debug)]
pub struct Receiver<T> {
    state: State<T>,
}

impl<T> Receiver<T> {
    pub fn close(&mut self) {
        let (data, task) = match self.state {
            State::Open(ref inner) => {
                let mut inner = inner.borrow_mut();
                drop(inner.rx_task.take());
                (inner.data.take(), inner.tx_task.take())
            }
            State::Closed(_) => return,
        };

        self.state = State::Closed(data);
        if let Some(task) = task {
            task.wake();
        }
    }
}

impl<T> Future for Receiver<T> {
    type Output = Result<T, Canceled>;

    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output> {
        let inner = match self.state {
            State::Open(ref mut inner) => inner,
            State::Closed(ref mut item) => match item.take() {
                Some(item) => return Poll::Ready(Ok(item.into())),
                None => return Poll::Ready(Err(Canceled)),
            },
        };

        if let Some(val) = inner.borrow_mut().data.take() {
            return Poll::Ready(Ok(val));
        }

        if Rc::get_mut(inner).is_some() {
            Poll::Ready(Err(Canceled))
        } else {
            inner.borrow_mut().rx_task = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

#[derive(Debug)]
pub struct Sender<T> {
    inner: Weak<RefCell<Inner<T>>>,
}

impl<T> Sender<T> {
    pub fn send(self, t: T) -> Result<(), T> {
        if let Some(inner) = self.inner.upgrade() {
            inner.borrow_mut().data = Some(t);
            Ok(())
        } else {
            Err(t)
        }
    }

    pub fn poll_cancel(&self, cx: &mut Context<'_>) -> Poll<()> {
        match self.inner.upgrade() {
            Some(inner) => {
                inner.borrow_mut().tx_task = Some(cx.waker().clone());
                Poll::Pending
            }
            None => Poll::Ready(()),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let inner = match self.inner.upgrade() {
            Some(inner) => inner,
            None => return,
        };

        let rx_task = {
            let mut borrow = inner.borrow_mut();
            borrow.tx_task.take();
            borrow.rx_task.take()
        };
        if let Some(task) = rx_task {
            task.wake();
        }
    }
}

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Rc::new(RefCell::new(Inner::new()));

    let sender = Sender {
        inner: Rc::downgrade(&inner),
    };

    let receiver = Receiver {
        state: State::Open(inner),
    };

    (sender, receiver)
}

impl<T> Unpin for Receiver<T> {}
impl<T> Unpin for Sender<T> {}
