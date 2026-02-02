use core::{borrow::Borrow, ops::Deref};

#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct RegisterFile {
    pub registers: [u64; 19],
    padding: u64,
    pub fp: u64,
    pub sp: u64,
    pub elr: usize,
    pub spsr: u64,
}

const _: () = assert!(size_of::<RegisterFile>() == 8 * 24);

#[derive(Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct RegisterFileRef<'a>(&'a mut RegisterFile);

impl RegisterFileRef<'_> {
    pub unsafe fn get_mut(&mut self) -> &mut RegisterFile {
        self.0
    }
}

impl AsRef<RegisterFile> for RegisterFileRef<'_> {
    fn as_ref(&self) -> &RegisterFile {
        self.0
    }
}

impl Borrow<RegisterFile> for RegisterFileRef<'_> {
    fn borrow(&self) -> &RegisterFile {
        self.0
    }
}

impl Deref for RegisterFileRef<'_> {
    type Target = RegisterFile;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

pub trait ExceptionHandler {
    extern "C" fn sync_current(register_file: RegisterFileRef) {
        _ = register_file;
        panic!("Unexpected sync exception from current EL");
    }

    extern "C" fn irq_current(register_file: RegisterFileRef) {
        _ = register_file;
        panic!("Unexpected IRQ from current EL");
    }

    extern "C" fn fiq_current(register_file: RegisterFileRef) {
        _ = register_file;
        panic!("Unexpected FIQ from current EL");
    }

    extern "C" fn serror_current(register_file: RegisterFileRef) {
        _ = register_file;
        panic!("Unexpected SError from current EL");
    }

    extern "C" fn sync_lower(register_file: RegisterFileRef) {
        _ = register_file;
        panic!("Unexpected sync exception from lower EL");
    }

    extern "C" fn irq_lower(register_file: RegisterFileRef) {
        _ = register_file;
        panic!("Unexpected IRQ from lower EL");
    }

    extern "C" fn fiq_lower(register_file: RegisterFileRef) {
        _ = register_file;
        panic!("Unexpected FIQ from lower EL");
    }

    extern "C" fn serror_lower(register_file: RegisterFileRef) {
        _ = register_file;
        panic!("Unexpected SError from lower EL");
    }
}

#[macro_export]
macro_rules! exception_handlers {
    ($handlers:ty) => {
        core::arch::global_asm!(
            r#"
.macro save_volatile_to_stack el:req
    stp x0, x1, [sp, #-(8 * 24)]!
    stp x2, x3, [sp, #8 * 2]
    stp x4, x5, [sp, #8 * 4]
    stp x6, x7, [sp, #8 * 6]
    stp x8, x9, [sp, #8 * 8]
    stp x10, x11, [sp, #8 * 10]
    stp x12, x13, [sp, #8 * 12]
    stp x14, x15, [sp, #8 * 14]
    stp x16, x17, [sp, #8 * 16]
    str x18, [sp, #8 * 18]
    stp x29, x30, [sp, #8 * 20]

    mrs x0, elr_\el
    mrs x1, spsr_\el
    stp x0, x1, [sp, #8 * 22]
.endm

.macro restore_volatile_from_stack el:req
    ldp x2, x3, [sp, #8 * 2]
    ldp x4, x5, [sp, #8 * 4]
    ldp x6, x7, [sp, #8 * 6]
    ldp x8, x9, [sp, #8 * 8]
    ldp x10, x11, [sp, #8 * 10]
    ldp x12, x13, [sp, #8 * 12]
    ldp x14, x15, [sp, #8 * 14]
    ldp x16, x17, [sp, #8 * 16]
    ldr x18, [sp, #8 * 18]
    ldp x29, x30, [sp, #8 * 20]

    ldp x0, x1, [sp, #8 * 22]
    msr elr_\el, x0
    msr spsr_\el, x1

    ldp x0, x1, [sp, #8 * 24]
.endm

.macro current_exception handler:req el:req
    save_volatile_to_stack \el
    mov x0, sp
    bl \handler
    restore_volatile_from_stack \el
    eret
.endm

.macro vector_table el:req
.section .text.vector_table_\el, "ax"
.global vector_table_\el
.balign 0x800
vector_table_\el:
sync_cur_sp0_\el:
    current_exception {sync_current} \el

.balign 0x80
irq_cur_sp0_\el:
    current_exception {irq_current} \el

.balign 0x80
fiq_cur_sp0_\el:
    current_exception {fiq_current} \el

.balign 0x80
serr_cur_sp0_\el:
    current_exception {serror_current} \el

.balign 0x80
sync_cur_spx_\el:
    current_exception {sync_current} \el

.balign 0x80
irq_cur_spx_\el:
    current_exception {irq_current} \el

.balign 0x80
fiq_cur_spx_\el:
    current_exception {fiq_current} \el

.balign 0x80
serr_cur_spx_\el:
    current_exception {serror_current} \el

.balign 0x80
sync_lower_64_\el:
    current_exception {sync_lower} \el

.balign 0x80
irq_lower_64_\el:
    current_exception {irq_lower} \el

.balign 0x80
fiq_lower_64_\el:
    current_exception {fiq_lower} \el

.balign 0x80
sync_lower_32_\el:
    current_exception {sync_lower} \el

.balign 0x80
irq_lower_32_\el:
    current_exception {irq_lower} \el

.balign 0x80
fiq_lower_32_\el:
    current_exception {fiq_lower} \el

.balign 0x80
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
