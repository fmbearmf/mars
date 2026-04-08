use core::{
    ops::Index,
    sync::atomic::{AtomicU8, Ordering},
};

use crate::{cpu_interface::Mpidr, scheduler::SCHEDULER};

use super::{cpu_interface::Arm64InterruptInterface, interrupt::gicv3::GicV3, sync::RwLock};

extern crate alloc;
use alloc::vec::Vec;

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

#[derive(Debug)]
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
    pub gic: Option<GicV3<'static, Arm64InterruptInterface>>,
    pub timer_irq: u64,
}

impl CpuDescriptor {
    pub const fn new() -> Self {
        Self {
            acpi_cpu_uid: u32::MAX,
            mpidr: u64::MAX,
            available: false,
            efficiency_class: 0,
            gic: None,
            timer_irq: 0_u64,
        }
    }
}

#[derive(Debug)]
pub struct VCpuList {
    pub cpus: Vec<CpuDescriptor>,
    pub fsms: Vec<CpuFsm>,
}

unsafe impl Send for VCpuList {}
unsafe impl Sync for VCpuList {}

impl VCpuList {
    pub const fn new() -> Self {
        Self {
            cpus: Vec::new(),
            fsms: Vec::new(),
        }
    }

    pub fn get_fsm(&self, vcpu: usize) -> Option<&CpuFsm> {
        self.fsms.get(vcpu)
    }

    pub fn get_fsm_mut(&mut self, vcpu: usize) -> Option<&mut CpuFsm> {
        self.fsms.get_mut(vcpu)
    }
}

const DEFAULT_FSM: CpuFsm = CpuFsm::new();

pub static VCPUS: RwLock<VCpuList> = RwLock::new(VCpuList::new());
//static VCPU_FSM: Vec<CpuFsm> = Vec::new();

pub fn add_cpu(cpu: CpuDescriptor) -> usize {
    let mut vcpus = VCPUS.write();

    SCHEDULER.register_cpu(cpu.mpidr);

    vcpus.cpus.push(cpu);

    let state = CpuFsm::new();
    state.advance();

    assert_eq!(state.load(), CpuState::Init);

    vcpus.fsms.push(state);

    vcpus.cpus.len()
}

pub fn with_this_cpu<F, R>(f: F) -> R
where
    F: FnOnce(&CpuDescriptor) -> R,
{
    let vcpus = VCPUS.read();
    let mpidr = Mpidr::current().affinity_only();
    let cpu = vcpus.cpus.get(mpidr as usize).expect("mpidr OOB");

    f(cpu)
}

pub fn with_cpus<F, R>(f: F) -> R
where
    F: FnOnce(usize, &[CpuDescriptor]) -> R,
{
    let vcpus = VCPUS.read();
    let count = vcpus.cpus.len();

    f(count, &vcpus.cpus[0..count])
}

pub fn vcpu_wait_init(vcpu: usize) {
    loop {
        let fsm_done = {
            let vcpus = VCPUS.read();
            match vcpus.get_fsm(vcpu) {
                Some(fsm) => fsm.load() == CpuState::Done,
                None => false,
            }
        };

        if fsm_done {
            break;
        }

        core::hint::spin_loop();
    }
}

pub fn vcpu_fsm_advance(vcpu: usize) -> CpuState {
    let vcpus = VCPUS.read();
    vcpus.get_fsm(vcpu).expect("vcpu index OOB").advance()
}
