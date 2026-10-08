#![no_std]
#![no_main]

extern crate alloc;

mod allocator;
mod elf;
mod page;

use core::mem::MaybeUninit;
use hal::paging::{AddressSpaceContext, MappingOptions};
use klib::{
    allocator_support::KernelAddressTranslator,
    pm::page::mapper::{AddressTranslator, MemoryProvider, map_region},
    vm::{PAGE_SIZE, align_down},
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
    page::{UefiAddressTranslator, map_identity, mmu_init},
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
        hal::boot::wait();
    }
}

#[allow(dead_code)]
fn busy_loop_noret() -> ! {
    loop {
        hal::boot::wait();
    }
}

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();

    debug!("main() @ {:#x}", main as *const () as usize);

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

    let kernel = match load_kernel(kernel) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let memory = MemoryProvider::new(&TABLE_ALLOC, &UefiAddressTranslator);
    let context = unsafe { AddressSpaceContext::create(&memory) }.unwrap();

    for segment in &kernel.segments {
        let root = context.table_for(segment.virtual_address, &memory).unwrap();
        unsafe {
            map_region(
                root,
                segment.physical_address,
                segment.virtual_address,
                segment.size,
                segment.options,
                &TABLE_ALLOC,
                &UefiAddressTranslator,
            )
        }
        .unwrap();
    }

    let st = uefi::table::system_table_raw().expect("no system table?");
    let raw_st = unsafe { st.as_ref() };
    let config = unsafe {
        core::slice::from_raw_parts(
            raw_st.configuration_table,
            raw_st.number_of_configuration_table_entries,
        )
    };
    let rsdp = config
        .iter()
        .find(|entry| entry.vendor_guid == uefi::table::cfg::ConfigTableEntry::ACPI2_GUID)
        .map(|entry| entry.vendor_table as usize);

    let Some(rsdp) = rsdp else {
        error!("ACPI 2.0 configuration table missing");
        return Status::NOT_FOUND;
    };

    let uart_phys = match unsafe { mars_acpi_driver::acpi::discover_pl011_uart(rsdp) } {
        Ok(address) if address.checked_add(0x1000).is_some() => {
            log::trace!("found PL011 at {:#x}", address);

            address
        }
        Ok(_) => {
            error!("SPCR UART physical range overflows");
            return Status::UNSUPPORTED;
        }
        Err(reason) => {
            error!("could not discover supported SPCR UART: {}", reason);

            0x40d_0000_usize
        }
    };

    let uart_phys_page = align_down(uart_phys, PAGE_SIZE);
    let root_ttbr0 = context.table_for(0, &memory).unwrap();
    let root_ttbr1 = context.table_for(usize::MAX, &memory).unwrap();

    unsafe { map_identity(root_ttbr0, uart_phys_page) }.unwrap();

    let entry_vaddr = kernel.entry;
    unsafe {
        map_region(
            root_ttbr1,
            uart_phys_page,
            KernelAddressTranslator.phys_to_dmap(uart_phys_page) as _,
            PAGE_SIZE,
            MappingOptions::MMIO,
            &TABLE_ALLOC,
            &UefiAddressTranslator,
        )
        .unwrap()
    };

    debug!("kernel entry: {:#x}", entry_vaddr);

    let mut boot_info = MaybeUninit::<BootInfo>::uninit();
    let mem_map_final = unsafe { boot::exit_boot_services(None) };

    unsafe {
        hal::boot::leave_firmware();
        mmu_init(context);
    }

    boot_info.write(BootInfo {
        kernel_load_physical_address: kernel.physical_address,
        kernel_size: kernel.size,
        serial_uart_address: uart_phys,
        memory_map: mem_map_final,
        system_table_raw: st,
        page_table_root: Some(root_ttbr0.as_ptr() as usize),
    });

    unsafe { hal::boot::enter_kernel(entry_vaddr, boot_info.as_mut_ptr() as usize) }
}
