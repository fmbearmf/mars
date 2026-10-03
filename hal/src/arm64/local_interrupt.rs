use core::arch::asm;

pub(crate) fn initialize() {
    unsafe {
        asm!(
            "msr ICC_SRE_EL1, {one}",
            "isb",
            "mrs {control}, ICC_CTLR_EL1",
            "orr {control}, {control}, #2",
            "msr ICC_CTLR_EL1, {control}",
            "msr ICC_BPR1_EL1, xzr",
            "isb",
            one = in(reg) 1u64,
            control = out(reg) _,
            options(nostack, preserves_flags)
        );
    }
}

pub(crate) fn acknowledge() -> Option<u32> {
    let value: u64;
    unsafe { asm!("mrs {0}, ICC_IAR1_EL1", out(reg) value, options(nostack, preserves_flags)) };
    let id = value as u32;
    (!(1020..=1023).contains(&id)).then_some(id)
}

pub(crate) fn complete(id: u32) {
    unsafe {
        asm!(
            "dsb sy",
            "msr ICC_EOIR1_EL1, {id}",
            "msr ICC_DIR_EL1, {id}",
            "isb",
            id = in(reg) u64::from(id),
            options(nostack, preserves_flags)
        );
    }
}

pub(crate) fn enable() {
    unsafe {
        asm!("msr ICC_IGRPEN1_EL1, {0}", "isb", in(reg) 1u64, options(nostack, preserves_flags))
    }
}

pub(crate) fn disable() {
    unsafe {
        asm!("msr ICC_IGRPEN1_EL1, {0}", "isb", in(reg) 0u64, options(nostack, preserves_flags))
    }
}

pub(crate) fn set_priority_limit(limit: u8) {
    unsafe {
        asm!("msr ICC_PMR_EL1, {0}", "isb", in(reg) u64::from(limit), options(nostack, preserves_flags))
    }
}
