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
use core::{
    arch::{asm, naked_asm},
    panic::PanicInfo,
    ptr,
};
use mars_klib::exception::ExceptionHandler;
use mars_protocol::BootInfo;

use crate::earlyinit::{
    earlycon::{EARLYCON, EarlyCon},
    mmu::init_mmu,
};

extern crate core;

struct Exceptions;
impl ExceptionHandler for Exceptions {}

mars_klib::exception_handlers!(Exceptions);

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    //unsafe {
    //    EARLYCON.force_unlock();
    //    earlycon_writeln!("{}", info);
    //}
    busy_loop();
}

fn busy_loop() -> ! {
    loop {
        wfe();
    }
}

fn busy_loop_ret() {
    loop {
        wfe();
    }
}

unsafe extern "C" {
    pub static __KBASE: usize;
}

const STACK_SIZE: usize = 128 * 1024;
#[repr(align(16))]
struct KStack([u8; STACK_SIZE]);

#[unsafe(link_section = ".reclaimable.bss")]
static mut KSTACK: KStack = KStack([0u8; STACK_SIZE]);

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start(_boot_info_ref: &mut BootInfo) {
    naked_asm!(
        "adrp x9, {stack_base}",
        "add x9, x9, :lo12:{stack_base}",
        //
        "add x9, x9, {stack_size}",
        "and x9, x9, #~0xF",
        "mov sp, x9",
        "b {entry}",
        stack_base = sym KSTACK,
        stack_size = const STACK_SIZE,
        entry = sym kentry,
    );
}

fn kentry(boot_info_ref: &mut BootInfo) -> ! {
    CPACR_EL1.modify(CPACR_EL1::FPEN::TrapNothing);
    CPACR_EL1.modify(CPACR_EL1::ZEN::TrapNothing);
    CPACR_EL1.modify(CPACR_EL1::TTA::NoTrap);
    isb(barrier::SY);

    let mpidr = MPIDR_EL1.get();
    let core_id = (mpidr & 0xFF) as u8;

    if core_id != 0 {
        busy_loop();
    }

    DAIF.write(DAIF::D::Masked + DAIF::A::Masked + DAIF::I::Masked + DAIF::F::Masked);

    let kbase = unsafe { &__KBASE as *const _ as usize };
    let offset = kbase - boot_info_ref.kernel_load_physical_address;

    let mut mmap = unsafe { ptr::read(&boot_info_ref.memory_map) };

    init_mmu(
        boot_info_ref.kernel_load_physical_address,
        offset,
        &mut mmap,
    )
}

pub extern "C" fn arm_init() {
    unsafe {
        asm!(
            "adr {x}, vector_table_el1",
            "msr vbar_el1, {x}",
            x = out(reg) _,
            options(nomem, nostack),
        );
    }

    busy_loop();

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
