use core::{fmt::Debug, marker::PhantomData};

/// 48-bit pointer.
#[repr(transparent)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct NicheKernelPtr<F> {
    bytes: [u8; 6],
    _phantom: PhantomData<F>,
}

impl<A, R> NicheKernelPtr<fn(A) -> R> {
    pub fn new(f: fn(A) -> R) -> Self {
        let f_usize = f as *const () as usize;

        let bytes = f_usize.to_le_bytes();
        let mut target = [0u8; 6];
        target.copy_from_slice(&bytes[0..6]);
        Self {
            bytes: target,
            _phantom: PhantomData,
        }
    }

    pub fn get(self) -> u64 {
        let mut bytes = [0u8; 8];
        bytes[0..6].copy_from_slice(&self.bytes);
        bytes[6] = 0xFF;
        bytes[7] = 0xFF;

        u64::from_le_bytes(bytes)
    }

    pub fn to_fn(self) -> fn(A) -> R {
        unsafe { core::mem::transmute(self.get()) }
    }
}
