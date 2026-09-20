use core::sync::atomic::{AtomicBool, Ordering};

use smccc::{
    Call, Hvc, Smc,
    psci::{PSCI_CPU_ON_64, version},
};

use super::cpu_interface::CpuTopologyId;

// pub const PSCI_0_2_FN64_CPU_ON: u32 = 0xC400_0003;
//
#[repr(i64)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PsciError {
    NotSupported = -1,
    InvalidParameters = -2,
    Denied = -3,
    AlreadyOn = -4,
    OnPending = -5,
    InternalFailure = -6,
    NotPresent = -7,
    Disabled = -8,
    InvalidAddress = -9,
    Unknown = i64::MAX,
}

impl PsciError {
    pub fn from_i64(code: i64) -> Result<(), PsciError> {
        match code as i32 {
            0 => Ok(()),
            -1 => Err(PsciError::NotSupported),
            -2 => Err(PsciError::InvalidParameters),
            -3 => Err(PsciError::Denied),
            -4 => Err(PsciError::AlreadyOn),
            -5 => Err(PsciError::OnPending),
            -6 => Err(PsciError::InternalFailure),
            -7 => Err(PsciError::NotPresent),
            -8 => Err(PsciError::Disabled),
            -9 => Err(PsciError::InvalidAddress),
            _ => Err(PsciError::Unknown),
        }
    }
}

pub static USE_HVC: AtomicBool = AtomicBool::new(false);

/// power on a CPU by its MPIDR using PSCI.
pub fn cpu_on(
    target_cpu: CpuTopologyId,
    entry_point_paddr: u64,
    context_id: u64,
) -> Result<(), PsciError> {
    let mut args = [0u64; 17];
    args[0] = target_cpu.to_mpidr();
    args[1] = entry_point_paddr;
    args[2] = context_id;

    let res = match USE_HVC.load(Ordering::Relaxed) {
        true => Hvc::call64(PSCI_CPU_ON_64, args),
        false => Smc::call64(PSCI_CPU_ON_64, args),
    };

    PsciError::from_i64(res[0] as i64)
}

pub fn print_psci_version() {
    use log::*;
    trace!("before SMC call");
    match version::<Smc>() {
        Ok(v) => trace!("PSCI_VERSION: {}.{}", v.major, v.minor),
        Err(e) => error!("failed to get PSCI version: {:?}", e),
    }
    trace!("after SMC call");
}
