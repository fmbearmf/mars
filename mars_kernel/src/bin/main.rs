#![no_std]
#![no_main]

mod earlyinit;

use aarch64_cpu::asm::wfe;
use core::{arch::asm, fmt::Write, panic::PanicInfo};
use mars_kernel::{
    exception::ExceptionHandler,
    fdt::Fdt,
    vm::{MemoryRegion, TABLE_ENTRIES},
};

use crate::earlyinit::{
    earlycon::{EARLYCON, EarlyCon},
    mmu::init_mmu,
};

extern crate core;

struct Exceptions;
impl ExceptionHandler for Exceptions {}

mars_kernel::exception_handlers!(Exceptions);

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe {
        EARLYCON.force_unlock();
        earlycon_writeln!("{}", info);
    }
    busy_loop();
}

fn busy_loop() -> ! {
    loop {
        wfe();
    }
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    core::arch::naked_asm!(
        "ldr x0, ={offset}",
        "ldr x30, =__boot_stack_top",
        "sub x30, x30, x0",
        "mov sp, x30",
        //
        "mrs x1, cpacr_el1",
        "orr x1, x1, #(0b11 << 20)",
        "msr cpacr_el1, x1",
        "isb",
        //
        "mrs x1, mpidr_el1", // core ID
        "and x1, x1, #0xFF", // check Aff0 (core ID)
        "cbnz x1, 1f",
        "ldr x1, =__bss_start",
        "ldr x2, =__bss_end",
        "sub x1, x1, x0",
        "sub x2, x2, x0",
        "2: cmp x1, x2",
        "b.ge 3f",
        "str xzr, [x1], #8",
        "b 2b",
        "3: ldr x0, ={lma}",
        "ldr x1, ={offset}",
        "msr daifset, #0b1111",
        "bl {setup}",
        "1: wfe",
        "b 1b",
        offset = sym crate::earlyinit::mmu::KERNEL_OFFSET,
        setup = sym crate::rust_entry,
        lma = sym crate::earlyinit::mmu::KERNEL_LOAD_PHYS_RAW,
    );
}

extern "C" fn rust_entry(load_addr: u64, offset: u64) -> ! {
    //{
    //    let mut lock = EARLYCON.lock();
    //    *lock = Some(EarlyCon::new());
    //}
    init_mmu(load_addr, offset)
}

pub extern "C" fn arm_init() {
    unsafe {
        asm!(
            "adr x9, vector_table_el1",
            "msr vbar_el1, x9",
            options(nomem, nostack),
            out("x9") _,
        );
    }

    {
        let mut lock = EARLYCON.lock();
        *lock = Some(EarlyCon::new());
    }

    unsafe {
        let dtb_addr = 0xFFFF_0000_0000_0000
            + (128 * 1024 * 1024 * 1024 * 1024)
            + ((TABLE_ENTRIES - 1) * (32 * 1024 * 1024));
        let fdt = Fdt::from_addr(dtb_addr).expect("invalid FDT");

        let mut regions: [MemoryRegion; 16] = [MemoryRegion { base: 0, size: 0 }; 16];
        let count = fdt
            .usable_mem_regions(&mut regions)
            .expect("failed to enum memory");

        for i in 0..count {
            let r = regions[i];
            earlycon_writeln!("region {} = base: {:#X}, size: {:#X}", i, r.base, r.size);
        }
    }

    //panic!("End.");
    busy_loop();
}
