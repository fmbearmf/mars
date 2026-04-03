use core::arch::asm;

use super::cpu_interface::Mpidr;

pub const PSCI_0_2_FN64_CPU_ON: u32 = 0xC400_0003;

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
        match code {
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

#[inline(always)]
unsafe fn smccc_call_hvc(fid: u32, arg1: u64, arg2: u64, arg3: u64) -> i64 {
    let res: i64;
    unsafe {
        asm!(
            "hvc #0",
            inlateout("x0") fid as u64 => res,
            in("x1") arg1,
            in("x2") arg2,
            in("x3") arg3,
            out("x4") _, out("x5") _, out("x6") _, out("x7") _,
            out("x8") _, out("x9") _, out("x10") _, out("x11") _,
            out("x12") _, out("x13") _, out("x14") _, out("x15") _,
            out("x16") _, out("x17") _,
            options(nostack)
        )
    };

    res
}

#[inline(always)]
unsafe fn smccc_call_smc(fid: u32, arg1: u64, arg2: u64, arg3: u64) -> i64 {
    let res: i64;
    unsafe {
        asm!(
            "smc #0",
            inlateout("x0") fid as u64 => res,
            in("x1") arg1,
            in("x2") arg2,
            in("x3") arg3,
            out("x4") _, out("x5") _, out("x6") _, out("x7") _,
            out("x8") _, out("x9") _, out("x10") _, out("x11") _,
            out("x12") _, out("x13") _, out("x14") _, out("x15") _,
            out("x16") _, out("x17") _,
            options(nostack)
        )
    };

    res
}

/// power on a CPU by its MPIDR using PSCI.
pub fn cpu_on(
    use_hvc: bool,
    target_cpu: Mpidr,
    entry_point_paddr: u64,
    context_id: u64,
) -> Result<(), PsciError> {
    let res = unsafe {
        if use_hvc {
            smccc_call_hvc(
                PSCI_0_2_FN64_CPU_ON,
                target_cpu.affinity_only(),
                entry_point_paddr,
                context_id,
            )
        } else {
            smccc_call_smc(
                PSCI_0_2_FN64_CPU_ON,
                target_cpu.affinity_only(),
                entry_point_paddr,
                context_id,
            )
        }
    };

    PsciError::from_i64(res)
}
