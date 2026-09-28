use alloc::{boxed::Box, string::String, sync::Arc, vec::Vec};

use crate::sync::{FairSpinlock, SleepingMutex};

use super::{BlockDevice, Consumer, HardwareAdapter, Provider};

pub type ProviderHandle = Arc<SleepingMutex<'static, dyn Provider>>;

static HARDWARE_DEVICES: FairSpinlock<Vec<ProviderHandle>> = FairSpinlock::new(Vec::new());

pub fn register_hardware_device(
    name: impl Into<String>,
    device: Box<dyn BlockDevice>,
) -> ProviderHandle {
    let handle: ProviderHandle = Arc::new(SleepingMutex::new(HardwareAdapter::new(name, device)));
    HARDWARE_DEVICES.lock().push(Arc::clone(&handle));
    handle
}

pub fn hardware_devices() -> Vec<ProviderHandle> {
    HARDWARE_DEVICES.lock().clone()
}

pub fn attach(handle: &ProviderHandle) -> Consumer {
    Consumer::attach(Arc::clone(handle))
}
