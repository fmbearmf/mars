use aarch64_cpu::registers::{ESR_EL1, FAR_EL1, Readable};

use super::{context::RegisterFileRef, cpu_interface::Mpidr};

pub trait ExceptionHandler {
    extern "C" fn sync_current(register_file: RegisterFileRef) -> RegisterFileRef {
        panic!(
            "Unexpected sync exception from CPU MPIDR={} (ESR: {:#x}, FAR: {:#x}) from current EL: {:?}",
            Mpidr::current().affinity_only(),
            ESR_EL1.get(),
            FAR_EL1.get(),
            register_file
        );
    }

    extern "C" fn irq_current(register_file: RegisterFileRef) -> RegisterFileRef {
        panic!(
            "Unexpected current EL IRQ from CPU MPIDR={} (FAR: {:#x}) from current EL: {:?}",
            Mpidr::current().affinity_only(),
            FAR_EL1.get(),
            register_file
        );
    }

    extern "C" fn fiq_current(register_file: RegisterFileRef) -> RegisterFileRef {
        panic!(
            "Unexpected current EL FIQ from CPU MPIDR={} (FAR: {:#x}) from current EL: {:?}",
            Mpidr::current().affinity_only(),
            FAR_EL1.get(),
            register_file
        );
    }

    extern "C" fn serror_current(register_file: RegisterFileRef) -> RegisterFileRef {
        _ = register_file;
        panic!("Unexpected SError from current EL");
    }

    extern "C" fn sync_lower(register_file: RegisterFileRef) -> RegisterFileRef {
        _ = register_file;
        panic!("Unexpected sync exception from lower EL");
    }

    extern "C" fn irq_lower(register_file: RegisterFileRef) -> RegisterFileRef {
        _ = register_file;
        panic!("Unexpected IRQ from lower EL");
    }

    extern "C" fn fiq_lower(register_file: RegisterFileRef) -> RegisterFileRef {
        _ = register_file;
        panic!("Unexpected FIQ from lower EL");
    }

    extern "C" fn serror_lower(register_file: RegisterFileRef) -> RegisterFileRef {
        _ = register_file;
        panic!("Unexpected SError from lower EL");
    }
}

#[macro_export]
macro_rules! exception_handlers {
    ($handlers:ty) => {
        core::arch::global_asm!(
            r#"
.macro save_regs el:req
    stp x0, x1, [sp, #-(8 * 34)]!

    stp x2, x3, [sp, #8 * 2]
    stp x4, x5, [sp, #8 * 4]
    stp x6, x7, [sp, #8 * 6]
    stp x8, x9, [sp, #8 * 8]
    stp x10, x11, [sp, #8 * 10]
    stp x12, x13, [sp, #8 * 12]
    stp x14, x15, [sp, #8 * 14]
    stp x16, x17, [sp, #8 * 16]
    stp x18, x19, [sp, #8 * 18]
    stp x20, x21, [sp, #8 * 20]
    stp x22, x23, [sp, #8 * 22]
    stp x24, x25, [sp, #8 * 24]
    stp x26, x27, [sp, #8 * 26]
    stp x28, x29, [sp, #8 * 28]
    str x30,      [sp, #8 * 30]

    add x2, sp, #(8 * 34)
    str x2, [sp, #8 * 31]

    mrs x0, elr_\el
    mrs x1, spsr_\el
    stp x0, x1, [sp, #8 * 32]
.endm

.macro restore_regs el:req
    ldp x0, x1, [sp, #8 * 32]
    msr elr_\el, x0
    msr spsr_\el, x1

    ldr x2,     [sp, #8 * 31]

    ldp x2, x3, [sp, #8 * 2]
    ldp x4, x5, [sp, #8 * 4]
    ldp x6, x7, [sp, #8 * 6]
    ldp x8, x9, [sp, #8 * 8]
    ldp x10, x11, [sp, #8 * 10]
    ldp x12, x13, [sp, #8 * 12]
    ldp x14, x15, [sp, #8 * 14]
    ldp x16, x17, [sp, #8 * 16]
    ldp x18, x19, [sp, #8 * 18]
    ldp x20, x21, [sp, #8 * 20]
    ldp x22, x23, [sp, #8 * 22]
    ldp x24, x25, [sp, #8 * 24]
    ldp x26, x27, [sp, #8 * 26]
    ldp x28, x29, [sp, #8 * 28]
    ldr x30,      [sp, #8 * 30]

    ldp x0, x1, [sp], #(8 * 34)
.endm

.macro current_exception handler:req el:req
    save_regs \el
    mov x0, sp
    bl \handler
    mov sp, x0
    restore_regs \el
    eret
.endm

.macro vector_table_entry label:req
.balign 0x80
    b \label
.endm

.macro vector_table el:req
.section .text.vector_table_\el, "ax"
.global vector_table_\el
.balign 0x800
vector_table_\el:

vector_table_entry sync_cur_sp0_\el
vector_table_entry irq_cur_sp0_\el
vector_table_entry fiq_cur_sp0_\el
vector_table_entry serr_cur_sp0_\el

vector_table_entry sync_cur_spx_\el
vector_table_entry irq_cur_spx_\el
vector_table_entry fiq_cur_spx_\el
vector_table_entry serr_cur_spx_\el

vector_table_entry sync_lower_64_\el
vector_table_entry irq_lower_64_\el
vector_table_entry fiq_lower_64_\el
vector_table_entry serr_lower_64_\el

vector_table_entry sync_lower_32_\el
vector_table_entry irq_lower_32_\el
vector_table_entry fiq_lower_32_\el
vector_table_entry serr_lower_32_\el

sync_cur_sp0_\el:
    current_exception {sync_current} \el
irq_cur_sp0_\el:
    current_exception {irq_current} \el
fiq_cur_sp0_\el:
    current_exception {fiq_current} \el
serr_cur_sp0_\el:
    current_exception {serror_current} \el

sync_cur_spx_\el:
    current_exception {sync_current} \el
irq_cur_spx_\el:
    current_exception {irq_current} \el
fiq_cur_spx_\el:
    current_exception {fiq_current} \el
serr_cur_spx_\el:
    current_exception {serror_current} \el

sync_lower_64_\el:
    current_exception {sync_lower} \el
irq_lower_64_\el:
    current_exception {irq_lower} \el
fiq_lower_64_\el:
    current_exception {fiq_lower} \el
serr_lower_64_\el:
    current_exception {serror_lower} \el

sync_lower_32_\el:
    current_exception {sync_lower} \el
irq_lower_32_\el:
    current_exception {irq_lower} \el
fiq_lower_32_\el:
    current_exception {fiq_lower} \el
serr_lower_32_\el:
    current_exception {serror_lower} \el

.endm

vector_table el1
            "#,
            sync_current = sym <$handlers as $crate::exception::ExceptionHandler>::sync_current,
            irq_current = sym <$handlers as $crate::exception::ExceptionHandler>::irq_current,
            fiq_current = sym <$handlers as $crate::exception::ExceptionHandler>::fiq_current,
            serror_current = sym <$handlers as $crate::exception::ExceptionHandler>::serror_current,
            sync_lower = sym <$handlers as $crate::exception::ExceptionHandler>::sync_lower,
            irq_lower = sym <$handlers as $crate::exception::ExceptionHandler>::irq_lower,
            fiq_lower = sym <$handlers as $crate::exception::ExceptionHandler>::fiq_lower,
            serror_lower = sym <$handlers as $crate::exception::ExceptionHandler>::serror_lower,
        );
    };
}
