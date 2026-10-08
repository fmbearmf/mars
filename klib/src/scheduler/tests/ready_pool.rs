extern crate std;

use super::super::ready_pool::ReadyPool;
use alloc::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::vec::Vec;
use std::{cell::Cell, rc::Rc, thread};

fn assert_send_sync<T: Send + Sync>() {}

trait AmbiguousIfSend<A> {
    fn marker() {}
}

impl<T: ?Sized> AmbiguousIfSend<()> for T {}
impl<T: ?Sized + Send> AmbiguousIfSend<*const ()> for T {}

trait AmbiguousIfSync<A> {
    fn marker() {}
}

impl<T: ?Sized> AmbiguousIfSync<()> for T {}
impl<T: ?Sized + Sync> AmbiguousIfSync<*const ()> for T {}

const _: () = {
    let _ = <ReadyPool<Rc<()>> as AmbiguousIfSend<_>>::marker;
    let _ = <ReadyPool<Rc<()>> as AmbiguousIfSync<_>>::marker;
    let _ = <ReadyPool<Cell<usize>> as AmbiguousIfSend<_>>::marker;
    let _ = <ReadyPool<Cell<usize>> as AmbiguousIfSync<_>>::marker;
};

#[test]
fn pool_send_sync_tracks_value_bounds() {
    assert_send_sync::<ReadyPool<usize>>();
}

#[test]
fn empty_pool_and_capacity_refill() {
    let pool = ReadyPool::new();
    assert!(!pool.has_work());
    assert!(pool.pop().is_none());

    for value in 0..256 {
        assert!(pool.push(Arc::new(value)).is_ok());
    }

    assert!(pool.has_work());

    let mut values: Vec<_> = (0..256).map(|_| *pool.pop().unwrap()).collect();
    values.sort_unstable();

    assert_eq!(values, (0..256).collect::<Vec<_>>());
    assert!(!pool.has_work());
    assert!(pool.pop().is_none());

    assert!(pool.push(Arc::new(256)).is_ok());
    assert_eq!(*pool.pop().unwrap(), 256);
}

#[test]
fn full_push_returns_arc_and_drop_releases_queued_arcs() {
    struct DropProbe(Arc<AtomicUsize>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    let drops = Arc::new(AtomicUsize::new(0));
    let pool = ReadyPool::new();
    let mut values = Vec::new();
    for _ in 0..256 {
        let value = Arc::new(DropProbe(Arc::clone(&drops)));
        values.push(Arc::clone(&value));
        assert!(pool.push(value).is_ok());
    }

    let rejected = Arc::new(DropProbe(Arc::clone(&drops)));
    let returned = pool.push(Arc::clone(&rejected)).unwrap_err();

    assert!(Arc::ptr_eq(&returned, &rejected));
    assert_eq!(Arc::strong_count(&returned), 2);

    drop(returned);
    drop(pool);

    assert_eq!(drops.load(Ordering::Relaxed), 0);

    drop(values);

    assert_eq!(drops.load(Ordering::Relaxed), 256);

    drop(rejected);

    assert_eq!(drops.load(Ordering::Relaxed), 257);
}

#[test]
fn concurrent_producers_and_thieves_deliver_each_id_once() {
    const PRODUCERS: usize = 4;
    const PER_PRODUCER: usize = 1_000;
    const TOTAL: usize = PRODUCERS * PER_PRODUCER;
    let pool = Arc::new(ReadyPool::new());
    let seen: Arc<Vec<AtomicUsize>> = Arc::new((0..TOTAL).map(|_| AtomicUsize::new(0)).collect());
    let consumed = Arc::new(AtomicUsize::new(0));
    let mut threads = Vec::new();

    for producer in 0..PRODUCERS {
        let pool = Arc::clone(&pool);
        threads.push(thread::spawn(move || {
            for offset in 0..PER_PRODUCER {
                let mut value = Arc::new(producer * PER_PRODUCER + offset);
                loop {
                    match pool.push(value) {
                        Ok(()) => break,
                        Err(returned) => {
                            value = returned;
                            thread::yield_now();
                        }
                    }
                }
            }
        }));
    }

    for _ in 0..4 {
        let pool = Arc::clone(&pool);
        let seen = Arc::clone(&seen);
        let consumed = Arc::clone(&consumed);
        threads.push(thread::spawn(move || {
            loop {
                if let Some(value) = pool.pop() {
                    assert_eq!(seen[*value].fetch_add(1, Ordering::Relaxed), 0);
                    consumed.fetch_add(1, Ordering::Release);
                } else if consumed.load(Ordering::Acquire) == TOTAL {
                    break;
                } else {
                    thread::yield_now();
                }
            }
        }));
    }

    for worker in threads {
        worker.join().unwrap();
    }
    assert_eq!(consumed.load(Ordering::Relaxed), TOTAL);
    assert!(seen.iter().all(|count| count.load(Ordering::Relaxed) == 1));
}
