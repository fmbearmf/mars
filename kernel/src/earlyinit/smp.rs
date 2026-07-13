use core::{
    arch::{asm, global_asm, naked_asm},
    sync::atomic::Ordering,
};

use aarch64_cpu::{
    asm::barrier::{self, dsb, isb},
    registers::{
        CPACR_EL1, DAIF, MAIR_EL1, ReadWriteable, Readable, SCTLR_EL1, TCR_EL1, TTBR0_EL1,
        TTBR1_EL1, Writeable,
    },
};
use aarch64_cpu_ext::asm::tlb::{VMALLE1, tlbi};
use alloc::boxed::Box;
use klib::{
    cache::clean_dcache_range,
    cpu_interface::{CpuIdLogical, CpuTopologyId},
    interrupt::InterruptController,
    per_cpu::PerCpu,
    pm::page::mapper::AddressTranslator,
    smccc::{PsciError, cpu_on},
    stack::Stack,
    this_cpu,
    timer::{init_timer, timer_rearm},
    vm::phys_addr_to_dmap,
};
use log::{info, trace};

use crate::{allocator::KernelAddressTranslator, busy_loop};

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
}

global_asm!(
    ".global smp_trampoline",
    ".section .text.smp_trampoline, \"ax\"",
    ".align 3",
    //
    "smp_trampoline:",
    "ldr x6, [x0, #0]",  // SecondaryBootArgs.stack_top (virtual)
    "ldr x7, [x0, #8]",  // SecondaryBootArgs.entry_fn (virtual)
    "ldr w8, [x0, #16]", // SecondaryBootArgs.cpu_id
    "ldr x1, [x0, #24]", // SecondaryBootArgs.ttbr0
    "ldr x2, [x0, #32]", // SecondaryBootArgs.ttbr1
    "ldr x3, [x0, #40]", // SecondaryBootArgs.tcr
    "ldr x4, [x0, #48]", // SecondaryBootArgs.mair
    "ldr x5, [x0, #56]", // SecondaryBootArgs.sctlr
    //
    "msr ttbr0_el1, x1",
    "msr ttbr1_el1, x2",
    "msr tcr_el1, x3",
    "msr mair_el1, x4",
    //
    "isb",
    //
    "msr sctlr_el1, x5",
    "isb",
    //
    "mov sp, x6",
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

    trace!("call cpu_on for {:?}", core.to_logical());

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
    info!("greetings from {}", cpu_id.to_u32());

    secondary_main();

    this_cpu!().ready.store(true, Ordering::Release);
    busy_loop()
}

fn secondary_main() {}

// #[unsafe(naked)]
// pub unsafe extern "C" fn secondary_entry(context: *const SecondaryBootArgs) -> ! {
//     naked_asm!(
//         "ldr x1, [x0, #0]",  // ttbr0
//         "ldr x2, [x0, #8]",  // ttbr1
//         "ldr x3, [x0, #16]", // tcr
//         "ldr x4, [x0, #24]", // mair
//         "ldr x5, [x0, #32]", // stack_top_virt
//         "ldr x6, [x0, #40]", // entry_virt
//         "ldr x7, [x0, #48]", // sctlr
//         "ldr x8, [x0, #56]", // cpudescriptor
//         "ldr x9, [x0, #64]", // gicd
//         //
//         "msr ttbr0_el1, x1",
//         "msr ttbr1_el1, x2",
//         "msr tcr_el1, x3",
//         "msr mair_el1, x4",
//         //
//         "dsb ish",
//         "tlbi vmalle1is",
//         "dsb sy",
//         "isb",
//         //
//         "msr sctlr_el1, x7",
//         "isb",
//         //
//         "mov sp, x5",
//         "br x6",
//     )
// }
//
// pub extern "C" fn secondary_init(context_phys: *const SecondaryBootArgs) -> ! {
//     unsafe {
//         asm!(
//             "adr {x}, vector_table_el1",
//             "msr vbar_el1, {x}",
//             x = out(reg) _,
//             options(nomem, nostack),
//         );
//     }
//
//     CPACR_EL1.modify(CPACR_EL1::FPEN::TrapNothing);
//     CPACR_EL1.modify(CPACR_EL1::ZEN::TrapNothing);
//     CPACR_EL1.modify(CPACR_EL1::TTA::NoTrap);
//     isb(barrier::SY);
//
//     DAIF.write(DAIF::D::Unmasked + DAIF::A::Unmasked + DAIF::I::Unmasked + DAIF::F::Unmasked);
//     isb(barrier::SY);
//
//     TTBR0_EL1.set_baddr(0);
//     TCR_EL1.modify(TCR_EL1::EPD0::DisableTTBR0Walks);
//     tlbi(VMALLE1);
//     dsb(barrier::ISH);
//     isb(barrier::SY);
//
//     // let context_ptr = phys_addr_to_dmap(context_phys as u64) as *const SecondaryBootArgs;
//
//     let gic = get_interrupt_controller();
//
//     gic.init().expect("gic init fail");
//     gic.enable_interrupt(
//         this_cpu!()
//             .timer_irq
//             .load(core::sync::atomic::Ordering::Relaxed) as _,
//     )
//     .expect("error enabling timer IRQ");
//
//     init_timer();
//     timer_rearm();
//
//     // let new_state = vcpu_fsm_advance(mpidr.to_mpidr() as usize);
//     // assert_eq!(new_state, CpuState::Done);
//
//     busy_loop()
// }
