use core::sync::atomic::{AtomicBool, Ordering};

use crate::{acpi::madt::CpuInfo, sync::Mutex};

pub const MAX_CPUS: usize = 256;

#[derive(Debug)]
pub struct VCpuList {
    pub cpus: [CpuInfo; MAX_CPUS],
    pub count: usize,
}

impl VCpuList {
    pub const fn new() -> Self {
        Self {
            cpus: [CpuInfo::new(); MAX_CPUS],
            count: 0,
        }
    }
}

const EMPTY_FLAG: AtomicBool = AtomicBool::new(false);

static VCPUS: Mutex<VCpuList> = Mutex::new(VCpuList::new());
static VCPU_INIT_FLAGS: [AtomicBool; MAX_CPUS] = [EMPTY_FLAG; MAX_CPUS];

pub fn add_cpu(cpu: CpuInfo) -> usize {
    let mut vcpus = VCPUS.lock();
    let count = vcpus.count;

    if count >= MAX_CPUS {
        panic!("ran out of CPU slots");
    }

    vcpus.cpus[count] = cpu;
    vcpus.count = count.saturating_add(1);

    VCPU_INIT_FLAGS[count].store(false, Ordering::Release);

    count
}

pub fn with_cpus<F, R>(f: F) -> R
where
    F: FnOnce(usize, &[CpuInfo]) -> R,
{
    let vcpus = VCPUS.lock();
    let count = vcpus.count;
    f(count, &vcpus.cpus[..count])
}

pub fn vcpu_wait_init(vcpu: usize) {
    while !VCPU_INIT_FLAGS[vcpu].load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
}

pub fn vcpu_signal_init(vcpu: usize) {
    VCPU_INIT_FLAGS[vcpu].store(true, Ordering::Release);
}
