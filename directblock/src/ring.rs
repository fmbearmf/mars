use core::{
    cell::UnsafeCell,
    fmt,
    marker::PhantomData,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use alloc::{boxed::Box, sync::Arc, vec::Vec};

#[derive(Debug, PartialEq, Eq)]
pub enum PushError<T> {
    /// ring buffer full
    Full(T),
    /// consumer has dropped. can't be processed
    Disconnected(T),
}

impl<T> PushError<T> {
    pub fn into_inner(self) -> T {
        match self {
            Self::Full(val) | Self::Disconnected(val) => val,
        }
    }
}

impl<T> fmt::Display for PushError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full(_) => write!(f, "queue is full"),
            Self::Disconnected(_) => write!(f, "consumer has disconnected"),
        }
    }
}

impl<T: fmt::Debug> core::error::Error for PushError<T> {}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PopError {
    /// the ring buffer is EMPTY.
    Empty,
    /// the producer has dropped and the remaining items have been consumed
    Disconnected,
}

impl fmt::Display for PopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "queue is empty"),
            Self::Disconnected => write!(f, "producer disconnected"),
        }
    }
}

impl core::error::Error for PopError {}

struct DropDrainGuard<'a, T, const CAP: usize> {
    buffer: &'a [UnsafeCell<MaybeUninit<T>>; CAP],
    tail: &'a AtomicUsize,
    cur: usize,
    end: usize,
}

impl<'a, T, const CAP: usize> Drop for DropDrainGuard<'a, T, CAP> {
    fn drop(&mut self) {
        self.tail.store(self.cur, Ordering::Release);
    }
}

impl<'a, T, const CAP: usize> DropDrainGuard<'a, T, CAP> {
    fn drain_all(&mut self) {
        let mask = self.buffer.len() - 1;

        while self.cur != self.end {
            let slot = self.cur & mask;
            self.cur = self.cur.wrapping_add(1);

            unsafe {
                core::ptr::drop_in_place(self.buffer[slot].get().cast::<T>());
            }
        }
    }
}

#[repr(align(64))]
struct ProducerState {
    head: AtomicUsize,
    is_alive: AtomicBool,
}

#[repr(align(64))]
struct ConsumerState {
    tail: AtomicUsize,
    is_alive: AtomicBool,
}

struct SharedRing<T, const CAP: usize> {
    buffer: Box<[UnsafeCell<MaybeUninit<T>>; CAP]>,
    producer: ProducerState,
    consumer: ConsumerState,
}

unsafe impl<T: Send, const CAP: usize> Send for SharedRing<T, CAP> {}
unsafe impl<T: Send, const CAP: usize> Sync for SharedRing<T, CAP> {}

impl<T, const CAP: usize> Drop for SharedRing<T, CAP> {
    fn drop(&mut self) {
        let tail = self.consumer.tail.load(Ordering::Acquire);
        let head = self.producer.head.load(Ordering::Acquire);
        let count = head.wrapping_sub(tail);

        assert!(count <= CAP, "ring state found corrupted during drop");

        if core::mem::needs_drop::<T>() {
            for i in 0..count {
                let slot = tail.wrapping_add(i) & (CAP - 1);
                unsafe {
                    core::ptr::drop_in_place(self.buffer[slot].get().cast::<T>());
                }
            }
        }
    }
}

pub struct Producer<T, const CAP: usize> {
    shared: Arc<SharedRing<T, CAP>>,
    local_head: usize,
    shadow_tail: usize,
    _not_sync: PhantomData<*const ()>,
}

unsafe impl<T: Send, const CAP: usize> Send for Producer<T, CAP> {}

pub struct Consumer<T, const CAP: usize> {
    shared: Arc<SharedRing<T, CAP>>,
    local_tail: usize,
    shadow_head: usize,
    _not_sync: PhantomData<*const ()>,
}

unsafe impl<T: Send, const CAP: usize> Send for Consumer<T, CAP> {}

struct CapacityAssert<const CAP: usize>;
impl<const CAP: usize> CapacityAssert<CAP> {
    const OK: () = {
        assert!(CAP >= 2, "capacity must be at least 2");
        assert!(CAP.is_power_of_two(), "capacity must be a power of two");
        assert!(CAP <= (usize::MAX >> 1) + 1, "capacity exceeds bounds");
    };
}

pub fn spsc_channel<T, const CAP: usize>() -> (Producer<T, CAP>, Consumer<T, CAP>) {
    let () = CapacityAssert::<CAP>::OK;

    let buffer: Box<[UnsafeCell<MaybeUninit<T>>; CAP]> = unsafe { Box::new_uninit().assume_init() };

    let shared = Arc::new(SharedRing {
        buffer,
        producer: ProducerState {
            head: AtomicUsize::new(0),
            is_alive: AtomicBool::new(true),
        },
        consumer: ConsumerState {
            tail: AtomicUsize::new(0),
            is_alive: AtomicBool::new(true),
        },
    });

    let producer = Producer {
        shared: Arc::clone(&shared),
        local_head: 0,
        shadow_tail: 0,
        _not_sync: PhantomData,
    };

    let consumer = Consumer {
        shared,
        local_tail: 0,
        shadow_head: 0,
        _not_sync: PhantomData,
    };

    (producer, consumer)
}

impl<T, const CAP: usize> Producer<T, CAP> {
    const MASK: usize = CAP - 1;

    pub fn push(&mut self, item: T) -> Result<(), PushError<T>> {
        if !self.shared.consumer.is_alive.load(Ordering::Acquire) {
            return Err(PushError::Disconnected(item));
        }

        let head = self.local_head;

        // fast path
        if head.wrapping_sub(self.shadow_tail) >= CAP {
            let actual_tail = self.shared.consumer.tail.load(Ordering::Acquire);
            self.shadow_tail = actual_tail;

            if !self.shared.consumer.is_alive.load(Ordering::Acquire) {
                return Err(PushError::Disconnected(item));
            }

            if head.wrapping_sub(actual_tail) >= CAP {
                return Err(PushError::Full(item));
            }
        }

        let slot = head & Self::MASK;
        unsafe {
            self.shared.buffer[slot].get().cast::<T>().write(item);
        }

        let next_head = head.wrapping_add(1);
        self.local_head = next_head;
        self.shared
            .producer
            .head
            .store(next_head, Ordering::Release);

        Ok(())
    }

    pub fn capacity(&self) -> usize {
        CAP
    }

    pub fn is_disconnected(&self) -> bool {
        !self.shared.consumer.is_alive.load(Ordering::Acquire)
    }

    pub fn len(&self) -> usize {
        let tail = self.shared.consumer.tail.load(Ordering::Acquire);
        self.local_head.wrapping_sub(tail)
    }

    pub fn is_full(&self) -> bool {
        self.len() >= CAP
    }
}

impl<T, const CAP: usize> Drop for Producer<T, CAP> {
    fn drop(&mut self) {
        self.shared
            .producer
            .is_alive
            .store(false, Ordering::Release);
    }
}

impl<T, const CAP: usize> Consumer<T, CAP> {
    const MASK: usize = CAP - 1;

    pub fn pop(&mut self) -> Result<T, PopError> {
        let tail = self.local_tail;

        // fast path
        if tail == self.shadow_head {
            let actual_head = self.shared.producer.head.load(Ordering::Acquire);
            self.shadow_head = actual_head;

            if tail == actual_head {
                if !self.shared.producer.is_alive.load(Ordering::Acquire) {
                    return Err(PopError::Disconnected);
                }

                return Err(PopError::Empty);
            }
        }

        let slot = tail & Self::MASK;
        let item = unsafe { self.shared.buffer[slot].get().cast::<T>().read() };

        let next_tail = tail.wrapping_add(1);
        self.local_tail = next_tail;
        self.shared
            .consumer
            .tail
            .store(next_tail, Ordering::Release);

        Ok(item)
    }

    pub fn capacity(&self) -> usize {
        CAP
    }

    pub fn is_disconnected(&self) -> bool {
        !self.shared.producer.is_alive.load(Ordering::Acquire)
    }

    pub fn len(&self) -> usize {
        let head = self.shared.producer.head.load(Ordering::Acquire);
        head.wrapping_sub(self.local_tail)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T, const CAP: usize> Drop for Consumer<T, CAP> {
    fn drop(&mut self) {
        self.shared
            .consumer
            .is_alive
            .store(false, Ordering::Release);

        let head = self.shared.producer.head.load(Ordering::Acquire);
        let tail = self.local_tail;
        let count = head.wrapping_sub(tail);

        if count <= CAP {
            if core::mem::needs_drop::<T>() {
                let mut guard = DropDrainGuard {
                    buffer: &self.shared.buffer,
                    tail: &self.shared.consumer.tail,
                    cur: tail,
                    end: head,
                };
                guard.drain_all();
            } else {
                self.shared.consumer.tail.store(head, Ordering::Release);
            }
        }
    }
}
