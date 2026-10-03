use aarch64_cpu::registers::{CurrentEL, Readable, VBAR_EL1, VBAR_EL2, Writeable};
use core::{
    arch::{asm, global_asm},
    sync::atomic::{AtomicPtr, Ordering},
};

use super::context::Context;
use crate::exception::{Exception, Handler};

static HANDLER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

pub(crate) unsafe fn install(handler: Handler) {
    let pointer = handler as *const () as *mut ();

    match HANDLER.compare_exchange(
        core::ptr::null_mut(),
        pointer,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => {}
        Err(installed) => assert_eq!(installed, pointer, "different exception handler installed"),
    }

    unsafe { install_vectors() };
}

pub(crate) unsafe fn install_vectors() {
    unsafe extern "C" {
        static hal_exception_vectors: u8;
    }

    let vectors = core::ptr::addr_of!(hal_exception_vectors) as u64;

    VBAR_EL1.set(vectors);

    if CurrentEL.read(CurrentEL::EL) == 2 {
        VBAR_EL2.set(vectors);
    }

    unsafe { asm!("isb", options(nostack, preserves_flags)) };
}

pub(crate) unsafe fn resume(context: &mut crate::context::Context) -> ! {
    let raw = context.raw_mut();

    assert!(
        !super::context::is_user(raw),
        "user context restore is unsupported"
    );

    unsafe { hal_resume_context(raw) }
}

unsafe extern "C" {
    fn hal_resume_context(context: *mut Context) -> !;
}

unsafe extern "C" fn dispatch(raw: *mut Context, kind: u64) {
    let handler = HANDLER.load(Ordering::Acquire);

    assert!(!handler.is_null(), "exception handler not installed");

    // assembly owns this live frame until the callback returns
    let context = unsafe { &mut *raw.cast::<crate::context::Context>() };
    let user = super::context::is_user(context.raw_mut());
    let exception = match kind {
        0 if !user && context.raw_mut().esr >> 26 == 0x15 => Exception::Reschedule,
        0 => Exception::Fault { user },
        1 => Exception::Interrupt,
        _ => Exception::Fatal,
    };

    let handler: Handler = unsafe { core::mem::transmute(handler) };

    unsafe { handler(exception, context) };

    // never apply a user stack to the kernel's exception return path
    assert!(
        !super::context::is_user(context.raw_mut()),
        "user context restore is unsupported"
    );
}

global_asm!(
    r#"
.section .text.hal_exception_vectors, "ax"
.balign 2048
.global hal_exception_vectors
hal_exception_vectors:
.macro vector target
    .balign 128
    b \target
.endm
    vector hal_bad_stack
    vector hal_bad_stack
    vector hal_bad_stack
    vector hal_bad_stack
    vector hal_sync_current
    vector hal_irq_current
    vector hal_fatal_current
    vector hal_fatal_current
    vector hal_sync_lower
    vector hal_irq_lower
    vector hal_fatal_lower
    vector hal_fatal_lower
    vector hal_fatal_lower
    vector hal_fatal_lower
    vector hal_fatal_lower
    vector hal_fatal_lower
.balign 128
hal_bad_stack:
    b hal_bad_stack

.macro save_frame lower
    .if \lower
        stp x0, x1, [sp, #-288]!
    .else
        stp x0, x1, [sp, #-416]!
    .endif
    stp x2, x3, [sp, #16]
    stp x4, x5, [sp, #32]
    stp x6, x7, [sp, #48]
    stp x8, x9, [sp, #64]
    stp x10, x11, [sp, #80]
    stp x12, x13, [sp, #96]
    stp x14, x15, [sp, #112]
    stp x16, x17, [sp, #128]
    stp x18, x19, [sp, #144]
    stp x20, x21, [sp, #160]
    stp x22, x23, [sp, #176]
    stp x24, x25, [sp, #192]
    stp x26, x27, [sp, #208]
    stp x28, x29, [sp, #224]
    str x30, [sp, #240]
    .if \lower
        mrs x16, sp_el0
    .else
        add x16, sp, #416
    .endif
    str x16, [sp, #248]
    mrs x16, elr_el1
    mrs x17, spsr_el1
    stp x16, x17, [sp, #256]
    mrs x16, esr_el1
    mrs x17, far_el1
    stp x16, x17, [sp, #272]
.endm

.macro exception_entry label, kind, lower
\label:
    save_frame \lower
    mov x0, sp
    mov x1, #\kind
    bl {dispatch}
    mov x0, sp
    b hal_resume_context
.endm
    exception_entry hal_sync_current, 0, 0
    exception_entry hal_irq_current, 1, 0
    exception_entry hal_fatal_current, 2, 0
    exception_entry hal_sync_lower, 0, 1
    exception_entry hal_irq_lower, 1, 1
    exception_entry hal_fatal_lower, 2, 1

.section .text.hal_resume_context, "ax"
.balign 16
.global hal_resume_context
hal_resume_context:
    // x0 points to a live frame. keep it in sp until the last pair loads
    mov sp, x0
    ldp x16, x17, [sp, #256]
    msr elr_el1, x16
    msr spsr_el1, x17
    ldp x2, x3, [sp, #16]
    ldp x4, x5, [sp, #32]
    ldp x6, x7, [sp, #48]
    ldp x8, x9, [sp, #64]
    ldp x10, x11, [sp, #80]
    ldp x12, x13, [sp, #96]
    ldp x14, x15, [sp, #112]
    ldp x16, x17, [sp, #128]
    ldp x18, x19, [sp, #144]
    ldp x20, x21, [sp, #160]
    ldp x22, x23, [sp, #176]
    ldp x24, x25, [sp, #192]
    ldp x26, x27, [sp, #208]
    ldp x28, x29, [sp, #224]
    ldr x30, [sp, #240]
    ldr x0, [sp, #248]
    mov x1, sp
    mov sp, x0
    ldp x0, x1, [x1]
    eret
"#,
    dispatch = sym dispatch,
);
