use crossbeam::atomic::AtomicCell;
use once_cell::sync::Lazy;

use mayastor::core::{
    mayastor_env_stop,
    poller,
    MayastorCliArgs,
    MayastorEnvironment,
    Reactors,
};
use std::cell::UnsafeCell;

pub mod common;

static COUNT: Lazy<AtomicCell<u32>> = Lazy::new(|| AtomicCell::new(0));

fn test_fn(a: u32, b: u32) -> i32 {
    let a = a + b;
    println!("{}", a);
    0
}

#[test]
fn poller() {
    common::mayastor_test_init();
    MayastorEnvironment::new(MayastorCliArgs::default()).init();

    let args = (1, 2);
    let poller = poller::Builder::new()
        .with_interval(0)
        .with_ctx(args)
        .with_poll_fn(|args| {
            println!("and a {} and {}", args.0, args.1);
            let mut count = COUNT.load();
            count += 1;
            COUNT.store(count);
            0
        })
        .build();

    drop(poller);

    Reactors::master().poll_once();

    // we dropped the poller before we polled, the value should still be 0
    assert_eq!(COUNT.load(), 0);

    let args = (1, 2);
    let mut poller = poller::Builder::new()
        .with_interval(0)
        .with_ctx(args)
        .with_poll_fn(move |_| {
            let count = COUNT.load();
            println!("and a {} and {} (count: {}) ", args.0, args.1, count);
            COUNT.store(count + 1);
            0
        })
        .build();

    Reactors::master().poll_times(64);
    assert_eq!(COUNT.load(), 64);

    poller.pause();
    Reactors::master().poll_times(64);
    assert_eq!(COUNT.load(), 64);

    poller.resume();
    Reactors::master().poll_times(64);
    assert_eq!(COUNT.load(), 128);

    poller.stop();
    Reactors::master().poll_times(64);
    assert_eq!(COUNT.load(), 128);

    let poller = poller::Builder::new()
        .with_interval(0)
        .with_ctx(0)
        .with_poll_fn(|_| test_fn(1, 2))
        .build();
    Reactors::master().poll_times(64);
    drop(poller);

    #[derive(Debug)]
    struct SomeArgs {
        f1: UnsafeCell<u64>,
        f2: u32,
    }

    let args = SomeArgs {
        f1: UnsafeCell::new(0),
        f2: 0,
    };

    let mut cnt: u32 = 0;
    let poller = poller::Builder::new()
        .with_interval(0)
        .with_ctx(args)
        .with_poll_fn(move |args| {
            unsafe {
                *args.f1.get() += 1;
                dbg!(*args.f1.get());
            };
            cnt += 1;
            dbg!(cnt);
            0
        })
        .build();

    Reactors::master().poll_times(64);

    mayastor_env_stop(0);
}
