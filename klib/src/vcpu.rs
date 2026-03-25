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

static VCPUS: Mutex<VCpuList> = Mutex::new(VCpuList::new());

pub fn add_cpu(cpu: CpuInfo) {
    let mut vcpus = VCPUS.lock();
    let count = vcpus.count;

    if count < MAX_CPUS {
        vcpus.cpus[count] = cpu;
        vcpus.count = count.checked_add(1).expect("ran out of CPU slots");
    }
}

pub fn with_cpus<F, R>(f: F) -> R
where
    F: FnOnce(usize, &[CpuInfo]) -> R,
{
    let vcpus = VCPUS.lock();
    let count = vcpus.count;
    f(count, &vcpus.cpus[..count])
}
