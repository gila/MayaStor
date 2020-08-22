use mayastor::core::{
    mayastor_env_stop,
    MayastorCliArgs,
    MayastorEnvironment,
    ReactorState,
    Reactors,
};
use std::mem;

pub mod common;

#[test]
fn thread_affinity() {
    common::mayastor_test_init();
    let mut args = MayastorCliArgs::default();
    args.reactor_mask = "0xF".into();
    let ms = MayastorEnvironment::new(args);

    ms.start(|| {
        Reactors::iter().for_each(|r| {
            assert_eq!(r.get_state(), ReactorState::Delayed);
        });

        let th = std::thread::spawn(|| {
            let mut set: libc::cpu_set_t =
                unsafe { mem::zeroed::<libc::cpu_set_t>() };
            unsafe { libc::CPU_SET(5, &mut set) };
            unsafe {
                libc::sched_setaffinity(
                    0, // Defaults to current thread
                    mem::size_of::<libc::cpu_set_t>(),
                    &set,
                );
            }
            println!("->  Thread on CPU {}", unsafe { libc::sched_getcpu() });
            let mut i = 0;

            while i < 10 {
                let cpu = unsafe { libc::sched_getcpu() };
                println!("thread {} (on CPU {})", i, cpu);
                i += 1;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        });

        let mut i = 0;
        while i < 10 {
            let cpu = unsafe { libc::sched_getcpu() };
            println!("main {} (on CPU {})", i, cpu);
            i += 1;
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        mayastor_env_stop(0);
    })
    .unwrap();
}
