use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::{acpi::madt::GicrFrame, interrupt::GicdRegisters, sync::Mutex};

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CpuState {
    Offline = 0,
    Init = 1,
    Done = 2,
}

impl CpuState {
    const fn from_u8(x: u8) -> Self {
        match x {
            0 => Self::Offline,
            1 => Self::Init,
            2 => Self::Done,
            _ => Self::Offline,
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Offline => Self::Init,
            Self::Init => Self::Done,
            Self::Done => Self::Done,
        }
    }

    const fn is_ready(self) -> bool {
        matches!(self, Self::Done)
    }
}

pub struct CpuFsm {
    state: AtomicU8,
}

impl CpuFsm {
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(CpuState::Offline as u8),
        }
    }

    pub fn load(&self) -> CpuState {
        CpuState::from_u8(self.state.load(Ordering::Acquire))
    }

    pub fn advance(&self) -> CpuState {
        let mut current = self.state.load(Ordering::Acquire);

        loop {
            let current_state = CpuState::from_u8(current);

            if current_state.is_ready() {
                return current_state;
            }

            let next = current_state.next() as u8;

            match self.state.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return CpuState::from_u8(next),
                Err(obs) => current = obs,
            }
        }
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub struct CpuDescriptor {
    pub acpi_cpu_uid: u32,
    pub mpidr: u64,
    pub available: bool,
    pub efficiency_class: u8,
    pub gicr: Option<GicrFrame<'static>>,
    pub timer_irq: u64,
}

impl CpuDescriptor {
    pub const fn new() -> Self {
        Self {
            acpi_cpu_uid: u32::MAX,
            mpidr: u64::MAX,
            available: false,
            efficiency_class: 0,
            gicr: None,
            timer_irq: 0_u64,
        }
    }
}

pub const MAX_CPUS: usize = 256;

#[derive(Debug)]
pub struct VCpuList {
    pub cpus: [CpuDescriptor; MAX_CPUS],
    pub count: usize,
}

impl VCpuList {
    pub const fn new() -> Self {
        Self {
            cpus: [CpuDescriptor::new(); MAX_CPUS],
            count: 0,
        }
    }
}

const DEFAULT_FSM: CpuFsm = CpuFsm::new();

static VCPUS: Mutex<VCpuList> = Mutex::new(VCpuList::new());
static VCPU_FSM: [CpuFsm; MAX_CPUS] = [DEFAULT_FSM; MAX_CPUS];

pub fn add_cpu(cpu: CpuDescriptor) -> usize {
    let mut vcpus = VCPUS.lock();
    let count = vcpus.count;

    if count >= MAX_CPUS {
        panic!("ran out of CPU slots");
    }

    vcpus.cpus[count] = cpu;
    vcpus.count = count.saturating_add(1);

    let new_state = VCPU_FSM[count].advance();
    assert_eq!(new_state, CpuState::Init);

    count
}

pub fn with_cpus<F, R>(f: F) -> R
where
    F: FnOnce(usize, &[CpuDescriptor]) -> R,
{
    let vcpus = VCPUS.lock();
    let count = vcpus.count;

    f(count, &vcpus.cpus[0..count])
}

pub fn vcpu_wait_init(vcpu: usize) {
    while !(VCPU_FSM[vcpu].load() == CpuState::Done) {
        core::hint::spin_loop();
    }
}

pub fn vcpu_fsm_advance(vcpu: usize) -> CpuState {
    VCPU_FSM[vcpu].advance()
}
