#![no_std]
#![no_main]
#![feature(stdarch_arm_hints)]
#![feature(fn_traits)]

extern crate alloc;

mod allocator;
mod elf;
mod page;

use core::{
    arch::aarch64::__wfe,
    mem::{MaybeUninit, transmute},
};

use aarch64_cpu::asm::barrier::{self, dsb};
use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
use klib::{
    pm::page::mapper::{TableAllocator, id_map, map_region},
    vm::{MAIR_DEVICE_INDEX, MAIR_NORMAL_INDEX, PAGE_SIZE, align_down, align_up},
};
use log::{debug, error, info};
use protocol::BootInfo;
use uefi::{
    CStr16, Status,
    allocator::Allocator,
    boot::{self},
    entry,
    proto::media::file::{File, FileAttribute, FileMode},
};

use crate::{
    allocator::UefiTableAlloc,
    elf::load_kernel,
    page::{UefiAddressTranslator, cpu_init, mmu_init},
};

#[global_allocator]
pub static EFI_ALLOC: Allocator = Allocator;

pub static TABLE_ALLOC: UefiTableAlloc = UefiTableAlloc;

const PT_LOAD: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Elf64Ehdr {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

bitflags::bitflags! {
    struct PhdrFlags: u32 {
        const EXEC = 0x1;
        const WRITE = 0x2;
        const READ = 0x4;
    }
}

#[allow(dead_code)]
fn busy_loop_ret() {
    loop {
        dsb(barrier::SY);
        unsafe { __wfe() };
    }
}

#[allow(dead_code)]
fn busy_loop_noret() -> ! {
    loop {
        dsb(barrier::SY);
        unsafe { __wfe() }
    }
}

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();

    debug!("main() @ {:#x}", main as *const () as usize);

    cpu_init();
    info!("Loader starting...");

    let mut sfs_prot = match boot::get_image_file_system(boot::image_handle()) {
        Ok(s) => s,
        Err(e) => {
            error!("get_image_file_system failed: {:?}", e);
            return Status::NOT_FOUND;
        }
    };

    let mut root_dir = match sfs_prot.open_volume() {
        Ok(d) => d,
        Err(e) => {
            error!("Couldn't open the root directory! {:?}", e);
            return Status::NOT_FOUND;
        }
    };

    let mut buf = [0u16; 12];
    let kpath = CStr16::from_str_with_buf("\\kernel.elf", &mut buf).expect("didnt fit");

    let fh = match root_dir.open(kpath, FileMode::Read, FileAttribute::empty()) {
        Ok(h) => h,
        Err(e) => {
            error!("Couldn't open \\kernel.elf: {:?}", e);
            return Status::NOT_FOUND;
        }
    };

    match fh.is_regular_file() {
        Ok(true) => {}
        Ok(false) => {
            error!("Kernel isn't a regular file!");
            return Status::UNSUPPORTED;
        }
        Err(e) => {
            error!("regular file check failed: {:?}", e);
            return Status::LOAD_ERROR;
        }
    };

    let kernel = fh.into_regular_file().unwrap();

    let (entry_offset, base_virt, base_phys, load_size) = match load_kernel(kernel) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let base_virt_align = align_down(base_virt as _, PAGE_SIZE);
    let base_phys_align = align_down(base_phys as _, PAGE_SIZE);
    let load_size_align = align_up(load_size as _, PAGE_SIZE);
    let entry_vaddr = base_virt_align + entry_offset as usize;

    debug!(
        "map {:#x}..{:#x} to {:#x}..{:#x}",
        base_virt_align,
        base_virt_align + load_size_align,
        base_phys_align,
        base_phys_align + load_size_align,
    );

    let mut root_ttbr1 = TABLE_ALLOC.alloc_table();
    debug!("root_ttbr1: {:p}", root_ttbr1);
    map_region::<_, UefiAddressTranslator>(
        unsafe { root_ttbr1.as_mut() },
        base_phys_align,
        base_virt_align,
        load_size_align,
        AccessPermission::PrivilegedReadWrite,
        Shareability::InnerShareable,
        true,
        false,
        MAIR_NORMAL_INDEX,
        &TABLE_ALLOC,
    );

    let mut root_ttbr0 = TABLE_ALLOC.alloc_table();
    debug!("root_ttbr0: {:p}", root_ttbr0);
    id_map::<_, UefiAddressTranslator>(
        unsafe { root_ttbr0.as_mut() },
        AccessPermission::PrivilegedReadWrite,
        Shareability::OuterShareable,
        true,
        false,
        MAIR_DEVICE_INDEX,
        &TABLE_ALLOC,
    );

    let entry_fn: fn(boot_info: *mut BootInfo) -> ! = unsafe { transmute(entry_vaddr) };
    debug!("entry_fn: {:p}", entry_fn as *const ());

    mmu_init(root_ttbr1.as_ptr());

    let mut boot_info = MaybeUninit::<BootInfo>::uninit();

    let mem_map_final = unsafe { boot::exit_boot_services(None) };

    boot_info.write(BootInfo {
        kernel_load_physical_address: base_phys as usize,
        kernel_size: load_size as usize,
        serial_uart_address: 0x0900_0000,
        memory_map: mem_map_final,
        page_table_root: Some(root_ttbr0.as_ptr()),
    });

    entry_fn(boot_info.as_mut_ptr());
}
