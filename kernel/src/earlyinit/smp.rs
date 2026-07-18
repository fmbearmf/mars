use core::{arch::global_asm, sync::atomic::Ordering};

use aarch64_cpu::registers::{
    CPACR_EL1, MAIR_EL1, Readable, SCTLR_EL1, TCR_EL1, TTBR0_EL1, TTBR1_EL1, VBAR_EL1,
};
use alloc::{boxed::Box, sync::Arc};
use klib::{
    cache::clean_dcache_range,
    context::RegisterFileRef,
    cpu_interface::{CpuIdLogical, CpuTopologyId},
    guard::InterruptGuard,
    per_cpu::PerCpu,
    pm::page::mapper::AddressTranslator,
    smccc::{PsciError, cpu_on},
    stack::Stack,
    this_cpu,
    thread::Thread,
};
use log::{info, trace};

use crate::{
    GLOBAL_SCHEDULER, allocator::KernelAddressTranslator, busy_loop, earlyinit::idle::idle_init,
    interrupt::get_interrupt_controller,
};

#[repr(C)]
#[derive(Debug)]
struct SecondaryBootArgs {
    pub stack_top_v: *mut (),
    pub entry_fn_v: *const (),
    //
    pub cpu_id: CpuIdLogical,
    pub ttbr0: u64,
    pub ttbr1: u64,
    pub tcr: u64,
    pub mair: u64,
    pub sctlr: u64,
    pub cpacr: u64,
    pub vbar: u64,
}

global_asm!(
    ".global smp_trampoline",
    ".section .text.smp_trampoline, \"ax\"",
    ".align 3",
    //
    "smp_trampoline:",
    "mrs x9, CurrentEL",
    "lsr x9, x9, #2",
    "cmp x9, #2",
    "b.ne .L_el1",
    //
    "mov x9, #(1 << 31)",
    "msr hcr_el2, x9",
    //
    "msr cptr_el2, xzr",
    //
    "mov x9, #0x3c5",
    "msr spsr_el2, x9",
    //
    "adr x9, .L_el1",
    "msr elr_el2, x9",
    //
    "msr cptr_el2, xzr",
    "msr hstr_el2, xzr",
    //
    "mov x9, #3",
    "msr cnthctl_el2, x9",
    "msr cntvoff_el2, xzr",
    //
    "dsb sy",
    "isb",
    //
    "eret",
    //
    ".L_el1:",
    "ldr x9, [x0, #0]",   // SecondaryBootArgs.stack_top (virtual)
    "ldr x7, [x0, #8]",   // SecondaryBootArgs.entry_fn (virtual)
    "ldr w8, [x0, #16]",  // SecondaryBootArgs.cpu_id
    "ldr x1, [x0, #24]",  // SecondaryBootArgs.ttbr0
    "ldr x2, [x0, #32]",  // SecondaryBootArgs.ttbr1
    "ldr x3, [x0, #40]",  // SecondaryBootArgs.tcr
    "ldr x4, [x0, #48]",  // SecondaryBootArgs.mair
    "ldr x5, [x0, #56]",  // SecondaryBootArgs.sctlr
    "ldr x6, [x0, #64]",  // SecondaryBootArgs.cpacr
    "ldr x10, [x0, #72]", // SecondaryBootArgs.cpacr
    //
    "msr ttbr0_el1, x1",
    "msr ttbr1_el1, x2",
    "msr tcr_el1, x3",
    "msr mair_el1, x4",
    //
    "isb",
    //
    "tlbi vmalle1",
    "dsb sy",
    "isb",
    //
    "msr sctlr_el1, x5",
    "msr cpacr_el1, x6",
    "msr vbar_el1, x10",
    "isb",
    //
    "mov sp, x9",
    "mov w0, w8",
    //
    "br x7",
);

unsafe extern "C" {
    pub fn smp_trampoline();
}

pub unsafe fn boot_secondary(
    core: CpuTopologyId,
    logical_id: CpuIdLogical,
    stack: Stack,
    addr_translator: impl Fn(usize) -> usize,
) -> Result<(), PsciError> {
    let stack_top = stack.as_ptr_range().end as usize;

    let mut args = Box::new(SecondaryBootArgs {
        stack_top_v: stack_top as _,
        entry_fn_v: secondary_init as _,
        cpu_id: logical_id,
        ttbr0: TTBR0_EL1.get(),
        ttbr1: TTBR1_EL1.get(),
        tcr: TCR_EL1.get(),
        mair: MAIR_EL1.get(),
        sctlr: SCTLR_EL1.get(),
        cpacr: CPACR_EL1.get(),
        vbar: VBAR_EL1.get(),
    });

    let args_ptr = args.as_mut() as *mut SecondaryBootArgs;
    unsafe {
        clean_dcache_range(
            args_ptr as *const _ as _,
            core::mem::size_of::<SecondaryBootArgs>(),
        )
    };

    let trampoline_phys = addr_translator(smp_trampoline as *const () as usize) as u64;
    let args_phys = KernelAddressTranslator.dmap_to_phys(args_ptr as _) as u64;

    trace!(
        "call cpu_on for {:?}, start at {:#x}",
        core.to_logical(),
        args_phys
    );

    cpu_on(core, trampoline_phys, args_phys)?;

    let pcpu = PerCpu::get(logical_id.to_usize()).expect("invalid logical_id passed");

    while pcpu.ready.load(Ordering::Acquire) != true {
        core::hint::spin_loop();
    }

    trace!("core {:?} woke up, moving on.", core.to_logical());

    Ok(())
}

#[allow(dead_code, reason = "called indirectly")]
pub unsafe extern "C" fn secondary_init(cpu_id: CpuIdLogical) -> ! {
    PerCpu::register_local(cpu_id.to_usize()).expect("invalid cpu_id passed to secondary core!");
    let _guard = InterruptGuard::new();

    info!("greetings from {}", cpu_id.to_u32());

    GLOBAL_SCHEDULER.register_cpu(cpu_id);

    secondary_main();

    idle_init()
}

fn secondary_main() {
    get_interrupt_controller().init().unwrap();
}
