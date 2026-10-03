#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(C, align(16))]
pub(crate) struct Context {
    pub(crate) registers: [u64; 31],
    pub(crate) sp: u64,
    pub(crate) elr: u64,
    pub(crate) spsr: u64,
    pub(crate) esr: u64,
    pub(crate) far: u64,
}

pub(crate) const fn stack_alignment() -> usize {
    16
}

pub(crate) unsafe fn kernel(entry: usize, stack_top: usize) -> Context {
    use aarch64_cpu::registers::Readable;
    let el = aarch64_cpu::registers::CurrentEL.read(aarch64_cpu::registers::CurrentEL::EL);
    let spsr = if el == 2 { 0x3c9 } else { 0x3c5 };
    Context {
        registers: [0; 31],
        sp: stack_top as u64,
        elr: entry as u64,
        spsr,
        esr: 0,
        far: 0,
    }
}

pub(crate) unsafe fn user(entry: usize, stack_top: usize) -> Context {
    Context {
        registers: [0; 31],
        sp: stack_top as u64,
        elr: entry as u64,
        spsr: 0,
        esr: 0,
        far: 0,
    }
}

pub(crate) fn is_user(context: &Context) -> bool {
    context.spsr & 0xf == 0
}

pub(crate) fn set_argument(context: &mut Context, argument: usize) {
    context.registers[0] = argument as u64;
}

const _: () = {
    assert!(core::mem::size_of::<Context>() == 288);
    assert!(core::mem::align_of::<Context>() == 16);
    assert!(core::mem::offset_of!(Context, registers) == 0);
    assert!(core::mem::offset_of!(Context, sp) == 248);
    assert!(core::mem::offset_of!(Context, elr) == 256);
    assert!(core::mem::offset_of!(Context, spsr) == 264);
    assert!(core::mem::offset_of!(Context, esr) == 272);
    assert!(core::mem::offset_of!(Context, far) == 280);
};

pub(crate) unsafe fn request_reschedule() {
    unsafe { core::arch::asm!("svc #0", options(nostack)) }
}
