#![no_std]
#![no_main]

mod earlyinit;

use aarch64_cpu::{
    asm::{
        barrier::{self, isb},
        wfe,
    },
    registers::{CPACR_EL1, DAIF, MPIDR_EL1, ReadWriteable, Readable, Writeable},
};
use core::{arch::asm, fmt::Write, panic::PanicInfo};
use mars_kernel::{
    exception::ExceptionHandler,
    fdt::Fdt,
    vm::{MemoryRegion, TABLE_ENTRIES},
};
use mars_protocol::BootInfo;

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

unsafe extern "C" {
    pub static __KBASE: usize;
}

#[unsafe(no_mangle)]
pub unsafe fn _start(boot_info: BootInfo) -> ! {
    CPACR_EL1.modify(CPACR_EL1::FPEN::TrapNothing);
    CPACR_EL1.modify(CPACR_EL1::ZEN::TrapNothing);
    CPACR_EL1.modify(CPACR_EL1::TTA::NoTrap);
    isb(barrier::SY);

    const FPEN_MASK: u64 = 0b11 << 20;

    unsafe {
        asm!(
            "mrs {tmp}, CPACR_EL1",
            "orr {tmp}, {tmp}, {mask}",
            "msr CPACR_EL1, {tmp}",
            "isb",
            tmp = out(reg) _,
            mask = const FPEN_MASK,
            options(nostack, preserves_flags, nomem),
        )
    };

    let mpidr = MPIDR_EL1.get();
    let core_id = (mpidr & 0xFF) as u8;

    if core_id == 0 {
        DAIF.write(DAIF::D::Masked + DAIF::A::Masked + DAIF::I::Masked + DAIF::F::Masked);

        rust_entry(&boot_info);
    }

    busy_loop()
}

extern "C" fn rust_entry(boot_info: &BootInfo) -> ! {
    //{
    //    let mut lock = EARLYCON.lock();
    //    *lock = Some(EarlyCon::new());
    //}
    let offset = unsafe { __KBASE } - boot_info.kernel_load_physical_address;

    init_mmu(
        boot_info.kernel_load_physical_address,
        offset,
        &boot_info.memory_map,
    )
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

    // unsafe {
    //     let dtb_addr = 0xFFFF_0000_0000_0000
    //         + (128 * 1024 * 1024 * 1024 * 1024)
    //         + ((TABLE_ENTRIES - 1) * (32 * 1024 * 1024));
    //     let fdt = Fdt::from_addr(dtb_addr).expect("invalid FDT");

    //     let mut regions: [MemoryRegion; 16] = [MemoryRegion { base: 0, size: 0 }; 16];
    //     let count = fdt
    //         .usable_mem_regions(&mut regions)
    //         .expect("failed to enum memory");

    //     for i in 0..count {
    //         let r = regions[i];
    //         earlycon_writeln!("region {} = base: {:#X}, size: {:#X}", i, r.base, r.size);
    //     }
    // }

    panic!("End.");
    busy_loop();
}
