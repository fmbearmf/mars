use core::{
    cell::UnsafeCell,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering},
};

use aarch64_cpu::registers::{DAIF, ReadWriteable, Readable, Writeable};

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

#[derive(Debug)]
pub struct RwLock<T: ?Sized> {
    state: AtomicI32,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Sync> Sync for RwLock<T> {}
unsafe impl<T: ?Sized + Send> Send for RwLock<T> {}

impl<T> RwLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            state: AtomicI32::new(0),
            data: UnsafeCell::new(value),
        }
    }
}

impl<T: ?Sized> RwLock<T> {
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        loop {
            let current = self.state.load(Ordering::Relaxed);

            if current >= 0 {
                if self
                    .state
                    .compare_exchange_weak(
                        current,
                        current + 1,
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    return RwLockReadGuard { lock: self };
                }
            }

            core::hint::spin_loop();
        }
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        loop {
            if self
                .state
                .compare_exchange_weak(0, -1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return RwLockWriteGuard { lock: self };
            }

            core::hint::spin_loop();
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
