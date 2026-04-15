use core::{
    cell::UnsafeCell,
    fmt::Debug,
    ops::{Deref, DerefMut},
    sync::atomic::{Atomic, AtomicBool, AtomicUsize, Ordering},
};

#[repr(C, align(64))]
#[derive(Debug)]
pub struct TicketLock {
    ticket: AtomicUsize,
    users: AtomicUsize,
}

impl TicketLock {
    pub const fn new() -> Self {
        Self {
            ticket: AtomicUsize::new(0),
            users: AtomicUsize::new(0),
        }
    }

    #[inline]
    pub fn lock(&self) {
        let ticket = self.ticket.fetch_add(1, Ordering::Relaxed);
        while self.users.load(Ordering::Acquire) != ticket {
            core::hint::spin_loop();
        }
    }

    #[inline]
    pub fn unlock(&self) {
        self.users.fetch_add(1, Ordering::Release);
    }
}

#[derive(Debug)]
pub struct Mutex<T> {
    lock: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: only 1 core can access `data` at a time
unsafe impl<T: Send> Sync for Mutex<T> {}
unsafe impl<T: Send> Send for Mutex<T> {}

pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
}

impl<T> Mutex<T> {
    pub const fn new(data: T) -> Self {
        Self {
            lock: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    #[inline]
    pub fn lock(&self) -> MutexGuard<'_, T> {
        while self
            .lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }

        MutexGuard { mutex: self }
    }

    #[inline]
    fn unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }
}

impl<'a, T> Deref for MutexGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<'a, T> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

type LockState = i32;

const WRITER: LockState = LockState::MIN;
const READERS: LockState = LockState::MAX;

#[derive(Debug)]
pub struct RwLock<T: ?Sized> {
    state: Atomic<LockState>,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Sync> Sync for RwLock<T> {}
unsafe impl<T: ?Sized + Send> Send for RwLock<T> {}

impl<T> RwLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            state: Atomic::<LockState>::new(0 as LockState),
            data: UnsafeCell::new(value),
        }
    }
}

impl<T: ?Sized> RwLock<T> {
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        let mut state = self.state.load(Ordering::Relaxed);

        loop {
            if state & WRITER == 0 {
                if state != READERS {
                    match self.state.compare_exchange_weak(
                        state,
                        state + 1,
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => return RwLockReadGuard { lock: self },
                        Err(next) => state = next,
                    }
                    continue;
                }
            }

            core::hint::spin_loop();
            state = self.state.load(Ordering::Relaxed);
        }
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        let mut state = self.state.load(Ordering::Relaxed);

        loop {
            if state & WRITER == 0 {
                match self.state.compare_exchange_weak(
                    state,
                    state | WRITER,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        if state == 0 {
                            return RwLockWriteGuard { lock: self };
                        }

                        while self.state.load(Ordering::Acquire) != WRITER {
                            core::hint::spin_loop();
                        }

                        return RwLockWriteGuard { lock: self };
                    }
                    Err(next) => state = next,
                }
            } else {
                core::hint::spin_loop();
                state = self.state.load(Ordering::Relaxed);
            }
        }
    }
}

pub struct RwLockReadGuard<'a, T: ?Sized> {
    lock: &'a RwLock<T>,
}

impl<'a, T: ?Sized> Deref for RwLockReadGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T: ?Sized> Drop for RwLockReadGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.state.fetch_sub(1, Ordering::Release);
    }
}

impl<'a, T: ?Sized + Debug> Debug for RwLockReadGuard<'a, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        (&**self).fmt(f)
    }
}

pub struct RwLockWriteGuard<'a, T: ?Sized> {
    lock: &'a RwLock<T>,
}

impl<'a, T: ?Sized> Deref for RwLockWriteGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T: ?Sized> DerefMut for RwLockWriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'a, T: ?Sized> Drop for RwLockWriteGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.state.store(0, Ordering::Release);
    }
}

impl<'a, T: ?Sized + Debug> Debug for RwLockWriteGuard<'a, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        (&**self).fmt(f)
    }
}
