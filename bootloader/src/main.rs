#![no_std]
#![no_main]
#![feature(stdarch_arm_hints)]
#![feature(fn_traits)]

extern crate alloc;

mod allocator;
mod elf;
mod page;
mod vec;

use core::{
    arch::aarch64::__wfe,
    mem::{self, MaybeUninit, transmute},
    ptr::{self},
    str::from_utf8,
};

use aarch64_cpu::asm::barrier::{self, dsb};
use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
use klib::{
    acpi::{
        self, SystemDescription,
        rsdp::{Rsdp, XsdtIter},
    },
    vec::{DynVec, RawVec, StaticVec},
    vm::{
        DMAP_START, MAIR_DEVICE_INDEX, MAIR_NORMAL_INDEX, MemoryRegion, MemoryRegionType,
        TTENATIVE, align_down, align_up,
        map::{TableAllocator, map_region},
    },
};
use log::{debug, error, info};
use protocol::BootInfo;
use uefi::{
    CStr16, Status,
    allocator::Allocator,
    boot::{self, MemoryType, PAGE_SIZE as UEFI_PAGE_SIZE},
    entry,
    mem::memory_map::MemoryMap,
    proto::media::file::{File, FileAttribute, FileMode},
};

use crate::{
    allocator::UefiPTAllocator,
    elf::load_kernel,
    page::{cpu_init, mmu_init},
    vec::UefiVec,
};

const PAGE_SIZE: usize = 16384;
const UEFI_PS: u64 = UEFI_PAGE_SIZE as u64;

#[global_allocator]
static EFI_ALLOC: Allocator = Allocator;

static PT_ALLOCATOR: UefiPTAllocator = UefiPTAllocator::new();

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
    let mut serial_uart_addr_opt = None;

    let mut kregions: UefiVec<MemoryRegion> = UefiVec::new();

    debug!("main() @ {:#x}", main as *const () as usize);

    cpu_init();
    info!("Loader starting...");

    let mut root_table = PT_ALLOCATOR.alloc_table();

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

    let (entry_addr, base_phys, load_size) = match load_kernel(kernel, root_table, &mut kregions) {
        Ok(v) => v,
        Err(e) => return e,
    };

    {
        let mem_map = boot::memory_map(MemoryType::LOADER_DATA).unwrap();

        {
            let rsdp = match Rsdp::find() {
                Ok(r) => r,
                Err(e) => {
                    error!("rsdp err: {}", e);
                    return Status::ABORTED;
                }
            };

            info!("RSDP at {:p}", rsdp);

            let xsdt = match rsdp.xsdt() {
                Ok(x) => x,
                Err(e) => {
                    error!("xsdt invalid: {}", e);
                    return Status::ABORTED;
                }
            };

            info!("XSDT found.");

            {
                let iter = XsdtIter::new(xsdt);
                for (i, table) in iter.enumerate() {
                    let sig = from_utf8(&table.sig).unwrap_or("ERR ");
                    let len = unsafe { ptr::read_unaligned(&raw const table.len) };
                    info!("     Table [{}]: \"{}\" ({} bytes)", i, sig, len);
                }
            }

            let sys = SystemDescription::parse(xsdt);

            if let Some(fadt) = sys.fadt {
                info!("FADT found. X_DSDT: {:#x}", sys.dsdt_addr);
                let arm_flags = fadt.arm_boot_arch();
                info!("arm boot arch: {:#06x}", arm_flags);
                if (arm_flags & 1) != 0 {
                    info!("PSCI compliant");
                }
            }

            if let Some(spcr) = sys.spcr {
                let base = spcr.base_addr.address();
                let type_ = spcr.interface_type();
                serial_uart_addr_opt = Some(base as usize);
                info!("SPCR serial type: {}, base: {:#x}", type_, base);
                let virt = TTENATIVE::align_down((DMAP_START as u64 + base) as u64) as usize;
                info!("mapping serial @ {:#x} to {:#x}", base, virt);
                map_region(
                    unsafe { root_table.as_mut() },
                    base as usize,
                    virt,
                    PAGE_SIZE * 1,
                    AccessPermission::PrivilegedReadWrite,
                    Shareability::OuterShareable,
                    true,
                    true,
                    MAIR_DEVICE_INDEX,
                    &PT_ALLOCATOR,
                );
            }

            if let Some(madt) = sys.madt {
                info!("MADT found");

                for (t, data) in madt.entries() {
                    match t {
                        acpi::madt::MADT_GICC => {
                            if data.len() >= mem::size_of::<acpi::madt::GicCpuInterface>() {
                                let gicc = unsafe {
                                    &*(data.as_ptr() as *const acpi::madt::GicCpuInterface)
                                };
                                let mpidr = unsafe { ptr::read_unaligned(&raw const gicc.mpidr) };
                                info!("     GICC CPU MPIDR={:#x}", mpidr);
                            }
                        }
                        acpi::madt::MADT_GICD => {
                            if data.len() >= mem::size_of::<acpi::madt::GicDistributor>() {
                                let gicd = unsafe {
                                    &*(data.as_ptr() as *const acpi::madt::GicDistributor)
                                };
                                let base =
                                    unsafe { ptr::read_unaligned(&raw const gicd.phys_base) };
                                info!("     GICD Distributor Base={:#x}", base);
                            }
                        }
                        acpi::madt::MADT_GICR => {
                            if data.len() >= mem::size_of::<acpi::madt::GicRedistributor>() {
                                let gicr = unsafe {
                                    &*(data.as_ptr() as *const acpi::madt::GicRedistributor)
                                };
                                let base = unsafe {
                                    ptr::read_unaligned(&raw const gicr.discovery_range_base)
                                };
                                info!("     GICR Redistributor Base={:#x}", base);
                            }
                        }
                        acpi::madt::MADT_ITS => {
                            if data.len() >= mem::size_of::<acpi::madt::GicIts>() {
                                let its = unsafe { &*(data.as_ptr() as *const acpi::madt::GicIts) };
                                let id =
                                    unsafe { ptr::read_unaligned(&raw const its.translation_id) };
                                let base = unsafe { ptr::read_unaligned(&raw const its.phys_base) };
                                info!("     ITS ID={}, Base={:#x}", id, base);
                            }
                        }
                        _ => {}
                    }
                }
            }

            if let Some(mcfg) = sys.mcfg {
                info!("MCFG found.");
                for alloc in mcfg.allocations() {
                    let base = unsafe { ptr::read_unaligned(&raw const alloc.base_addr) };
                    let seg = unsafe { ptr::read_unaligned(&raw const alloc.pci_segment_group) };
                    let start = unsafe { ptr::read_unaligned(&raw const alloc.start_bus_num) };
                    let end = unsafe { ptr::read_unaligned(&raw const alloc.end_bus_num) };
                    info!("PCI seg {} Base={:#x}, Bus {}-{}", seg, base, start, end);
                }
            }

            if let Some(gtdt) = sys.gtdt {
                let virt = gtdt.virt_el1_gsiv();
                let phys = gtdt.ns_el1_gsiv();
                info!("GTDT timer GSIVs Virt={}, Phys={}", virt, phys);
            }
        }

        for entry in mem_map.entries() {
            let phys_start_unaligned = entry.phys_start as usize;
            let phys_start = align_down(phys_start_unaligned, PAGE_SIZE);

            let size_unaligned = (entry.page_count * UEFI_PS) as usize;
            let size = align_down(size_unaligned, PAGE_SIZE);
            let vaddr = align_up(DMAP_START + phys_start, PAGE_SIZE);

            //info!(
            //    "start: {:#x}, start_align: {:#x}, size: {:#x}, size_align: {:#x}",
            //    phys_start_unaligned, phys_start, size_unaligned, size
            //);

            if size < PAGE_SIZE || phys_start + size > phys_start_unaligned + size_unaligned {
                continue;
            }

            if let Some(uart) = serial_uart_addr_opt {
                if phys_start > uart && phys_start < (uart + PAGE_SIZE) {
                    info!("alert!");
                    busy_loop_ret();
                }
            }

            let region_type = match entry.ty {
                MemoryType::LOADER_CODE => MemoryRegionType::BootloaderReclaim,
                MemoryType::BOOT_SERVICES_CODE => MemoryRegionType::FirmwareReclaim,
                MemoryType::RUNTIME_SERVICES_CODE => MemoryRegionType::RtFirmwareCode,
                MemoryType::MMIO | MemoryType::MMIO_PORT_SPACE => MemoryRegionType::Mmio,
                _ => MemoryRegionType::Unknown,
            };

            match entry.ty {
                // RW no exec
                //MemoryType::CONVENTIONAL
                //MemoryType::LOADER_DATA |
                | MemoryType::LOADER_CODE
                //| MemoryType::BOOT_SERVICES_DATA
                //| MemoryType::BOOT_SERVICES_CODE
                //| MemoryType::RUNTIME_SERVICES_DATA
                => {
                    info!(
                        "MemoryDescriptor {{ phys_start: {:#x}, size: {:#x}, ty: {:?} }}",
                        entry.phys_start,
                        entry.page_count * UEFI_PS,
                        entry.ty
                    );
                    map_region(
                        unsafe { root_table.as_mut() },
                        phys_start,
                        vaddr,
                        size,
                        AccessPermission::PrivilegedReadWrite,
                        Shareability::OuterShareable,
                        true,
                        true,
                        MAIR_NORMAL_INDEX,
                        &PT_ALLOCATOR,
                    );
                    kregions.push(MemoryRegion { base: phys_start, size, region_type });
                }

                // RO exec
                //MemoryType::LOADER_CODE |
                //MemoryType::BOOT_SERVICES_CODE |
                //MemoryType::RUNTIME_SERVICES_CODE => {
                //    map_region(
                //        unsafe { root_table.as_mut() },
                //        phys_start,
                //        vaddr,
                //        size,
                //        AccessPermission::PrivilegedReadOnly,
                //        Shareability::InnerShareable,
                //        true,
                //        false,
                //        MAIR_NORMAL_INDEX,
                //        &PT_ALLOCATOR,
                //    );
                //    kregions.push(MemoryRegion { base: vaddr, size, region_type });
                //}

                // RO no exec
                MemoryType::ACPI_RECLAIM => {
                    //map_region(
                    //    unsafe { root_table.as_mut() },
                    //    phys_start,
                    //    vaddr,
                    //    size,
                    //    AccessPermission::PrivilegedReadOnly,
                    //    Shareability::InnerShareable,
                    //    true,
                    //    true,
                    //    MAIR_NORMAL_INDEX,
                    //    &allocator,
                    //);
                }

                // RO no exec device
                MemoryType::ACPI_NON_VOLATILE => {
                    //map_region(
                    //    unsafe { root_table.as_mut() },
                    //    phys_start,
                    //    vaddr,
                    //    size,
                    //    AccessPermission::PrivilegedReadOnly,
                    //    Shareability::OuterShareable,
                    //    true,
                    //    true,
                    //    MAIR_DEVICE_INDEX,
                    //    &allocator,
                    //);
                }

                // RW no exec device
                MemoryType::MMIO | MemoryType::MMIO_PORT_SPACE => {
                    map_region(
                        unsafe { root_table.as_mut() },
                        phys_start,
                        vaddr,
                        size,
                        AccessPermission::PrivilegedReadWrite,
                        Shareability::OuterShareable,
                        true,
                        true,
                        MAIR_DEVICE_INDEX,
                        &PT_ALLOCATOR,
                    );
                    kregions.push(MemoryRegion { base: phys_start, size, region_type });
                }

                _ => {
                    //info!("Unrecognized UEFI mem map entry type: {:?}", entry);
                }
            }
        }
    }

    mmu_init(root_table.as_ptr());

    let serial_uart_addr = serial_uart_addr_opt.unwrap();

    let entry_fn: fn(boot_info: MaybeUninit<BootInfo>) -> ! = unsafe { transmute(entry_addr) };

    let mut boot_info = MaybeUninit::<BootInfo>::uninit();

    //kregions.extend(PT_ALLOCATOR.take_kernel_regions());
    kregions.compact();

    let mem_map_final = unsafe { boot::exit_boot_services(None) };

    boot_info.write(BootInfo {
        kernel_load_physical_address: base_phys as usize,
        kernel_size: load_size as usize,
        serial_uart_address: serial_uart_addr + DMAP_START,
        memory_map: mem_map_final,
        root_pt: root_table,
        kernel_regions: core::ops::Fn::call(&StaticVec::from_raw_parts, kregions.into_raw_parts()),
    });

    entry_fn(boot_info);
}
