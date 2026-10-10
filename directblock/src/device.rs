use core::sync::atomic::{AtomicU64, Ordering};

use alloc::{boxed::Box, sync::Arc};

use crate::{
    IoError, IoResult,
    bio::{Bio, SubmitError},
};

#[derive(Debug, Copy, Clone)]
pub struct DeviceInfo {
    pub block_size: u32,
    pub block_count: u64,
}

pub trait Device: Send + Sync {
    fn info(&self) -> DeviceInfo;
    fn open_channel(&self) -> Result<Box<dyn Channel>, IoError>;
}

pub trait Channel: Send {
    fn submit(&mut self, bio: Bio) -> Result<(), SubmitError>;

    fn poll(&mut self, budget: usize) -> usize {
        let _ = budget;

        0
    }
}

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ProviderState {
    Created = 0,
    Online,
    Quiescing,
    Offline,
    Failed,
}

const STATE_BITS: u64 = 3;
const STATE_MASK: u64 = (1 << STATE_BITS) - 1;
const ONE_FLIGHT: u64 = 1 << STATE_BITS;

fn decode_state(value: u64) -> ProviderState {
    match value & STATE_MASK {
        0 => ProviderState::Created,
        1 => ProviderState::Online,
        2 => ProviderState::Quiescing,
        3 => ProviderState::Offline,
        4 => ProviderState::Failed,
        _ => unreachable!(),
    }
}

pub struct Provider {
    device: Arc<dyn Device>,
    word: AtomicU64,
}

impl Provider {
    pub fn new(device: Arc<dyn Device>) -> Arc<Self> {
        Arc::new(Self {
            device,
            word: AtomicU64::new(ProviderState::Created as u64),
        })
    }

    pub fn info(&self) -> DeviceInfo {
        self.device.info()
    }

    pub fn state(&self) -> ProviderState {
        decode_state(self.word.load(Ordering::Acquire))
    }

    pub fn in_flight(&self) -> u64 {
        self.word.load(Ordering::Acquire) >> STATE_BITS
    }

    fn transition(&self, from: ProviderState, to: ProviderState) -> IoResult {
        let mut old = self.word.load(Ordering::Acquire);

        loop {
            if decode_state(old) != from {
                return Err(IoError::Busy);
            }

            let new = (old & !STATE_MASK) | to as u64;

            match self
                .word
                .compare_exchange_weak(old, new, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Ok(()),
                Err(actual) => old = actual,
            }
        }
    }

    pub fn publish(&self) -> IoResult {
        self.transition(ProviderState::Created, ProviderState::Online)
    }

    pub fn quiesce(&self) -> IoResult {
        self.transition(ProviderState::Online, ProviderState::Quiescing)
    }

    pub fn fail(&self) -> IoResult {
        let mut old = self.word.load(Ordering::Acquire);

        loop {
            match decode_state(old) {
                ProviderState::Created | ProviderState::Online | ProviderState::Quiescing => {}
                ProviderState::Failed => return Ok(()),
                ProviderState::Offline => {
                    return Err(IoError::Offline);
                }
            }

            let new = (old & !STATE_MASK) | ProviderState::Failed as u64;

            match self
                .word
                .compare_exchange_weak(old, new, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Ok(()),
                Err(actual) => old = actual,
            }
        }
    }

    pub fn try_offline(&self) -> IoResult {
        let mut old = self.word.load(Ordering::Acquire);

        loop {
            match decode_state(old) {
                ProviderState::Quiescing | ProviderState::Failed => {}
                _ => return Err(IoError::Busy),
            }

            if old >> STATE_BITS != 0 {
                return Err(IoError::Busy);
            }

            let new = ProviderState::Offline as u64;

            match self
                .word
                .compare_exchange_weak(old, new, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Ok(()),
                Err(actual) => old = actual,
            }
        }
    }

    fn enter(self: &Arc<Self>) -> Result<Flight, IoError> {
        let mut old = self.word.load(Ordering::Acquire);

        loop {
            match decode_state(old) {
                ProviderState::Online => {}
                ProviderState::Created => {
                    return Err(IoError::NotReady);
                }
                ProviderState::Quiescing => {
                    return Err(IoError::Quiescing);
                }
                ProviderState::Offline => {
                    return Err(IoError::Offline);
                }
                ProviderState::Failed => {
                    return Err(IoError::DeviceFailed);
                }
            }

            if old >> STATE_BITS == (u64::MAX >> STATE_BITS) {
                return Err(IoError::ResourceExhausted);
            }

            match self.word.compare_exchange_weak(
                old,
                old + ONE_FLIGHT,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(Flight {
                        provider: Arc::clone(self),
                    });
                }
                Err(actual) => old = actual,
            }
        }
    }

    fn leave(&self) {
        let old = self.word.fetch_sub(ONE_FLIGHT, Ordering::AcqRel);

        debug_assert_ne!(old >> STATE_BITS, 0);
    }

    pub fn open_channel(self: &Arc<Self>) -> Result<Box<dyn Channel>, IoError> {
        if self.state() != ProviderState::Online {
            return Err(IoError::NotReady);
        }

        let inner = self.device.open_channel()?;

        Ok(Box::new({
            ProviderChannel {
                provider: Arc::clone(self),
                inner,
            }
        }))
    }
}

/// lease of one admitted request.
pub(crate) struct Flight {
    provider: Arc<Provider>,
}

impl Drop for Flight {
    fn drop(&mut self) {
        self.provider.leave();
    }
}

struct ProviderChannel {
    provider: Arc<Provider>,
    inner: Box<dyn Channel>,
}

impl Channel for ProviderChannel {
    fn submit(&mut self, mut bio: Bio) -> Result<(), SubmitError> {
        let lease = match self.provider.enter() {
            Ok(lease) => lease,
            Err(e) => {
                return Err(SubmitError::Failed(e, bio));
            }
        };

        if let Err(e) = bio.validate(self.provider.info()) {
            drop(lease);

            return Err(SubmitError::Failed(e, bio));
        }

        bio.completion.attach(lease);

        match self.inner.submit(bio) {
            Ok(()) => Ok(()),
            Err(SubmitError::Full(mut bio)) => {
                bio.completion.detach();
                Err(SubmitError::Full(bio))
            }
            Err(SubmitError::Failed(error, mut bio)) => {
                bio.completion.detach();
                Err(SubmitError::Failed(error, bio))
            }
        }
    }

    fn poll(&mut self, budget: usize) -> usize {
        self.inner.poll(budget)
    }
}
