use core::{
    marker::PhantomData,
    sync::atomic::{AtomicPtr, AtomicUsize, Ordering},
};

use alloc::sync::Arc;

const READY_POOL_SIZE: usize = 256;

/// bounded lockfree pool
pub(super) struct ReadyPool<T> {
    slots: [AtomicPtr<T>; READY_POOL_SIZE],
    cursor: AtomicUsize,
    _value: PhantomData<Arc<T>>,
}

impl<T> ReadyPool<T> {
    pub(super) const fn new() -> Self {
        Self {
            slots: [const { AtomicPtr::new(core::ptr::null_mut()) }; READY_POOL_SIZE],
            cursor: AtomicUsize::new(0),
            _value: PhantomData,
        }
    }

    pub(super) fn push(&self, value: Arc<T>) -> Result<(), Arc<T>> {
        let start = self.cursor.fetch_add(1, Ordering::Relaxed);
        let pointer = Arc::into_raw(value).cast_mut();

        for offset in 0..READY_POOL_SIZE {
            let slot = &self.slots[(start.wrapping_add(offset)) % READY_POOL_SIZE];
            if slot
                .compare_exchange(
                    core::ptr::null_mut(),
                    pointer,
                    Ordering::Release,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                return Ok(());
            }
        }

        // safety: no slot accepted the pointer, so this function still owns its raw strong reference
        Err(unsafe { Arc::from_raw(pointer) })
    }

    pub(super) fn len(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| !slot.load(Ordering::Acquire).is_null())
            .count()
    }

    pub(super) fn has_work(&self) -> bool {
        self.slots
            .iter()
            .any(|slot| !slot.load(Ordering::Acquire).is_null())
    }

    pub(super) fn pop(&self) -> Option<Arc<T>> {
        let start = self.cursor.fetch_add(1, Ordering::Relaxed);
        for offset in 0..READY_POOL_SIZE {
            let slot = &self.slots[(start.wrapping_add(offset)) % READY_POOL_SIZE];
            let pointer = slot.swap(core::ptr::null_mut(), Ordering::Acquire);
            if !pointer.is_null() {
                // safety: this swap removed the unique strong reference published in the slot
                return Some(unsafe { Arc::from_raw(pointer) });
            }
        }

        None
    }
}

impl<T> Drop for ReadyPool<T> {
    fn drop(&mut self) {
        for slot in &self.slots {
            let pointer = slot.swap(core::ptr::null_mut(), Ordering::Acquire);
            if !pointer.is_null() {
                // safety: exclusive access during drop follows the same ownership transfer as pop
                drop(unsafe { Arc::from_raw(pointer) });
            }
        }
    }
}
