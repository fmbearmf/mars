#![no_std]
#![no_main]
#![feature(stdarch_arm_hints)]

extern crate alloc;

mod page;

use core::{
    alloc::Layout,
    arch::aarch64::__wfe,
    mem::{self, transmute},
    ops::Div,
    ptr::{self, NonNull, copy_nonoverlapping, write_volatile},
    slice::from_raw_parts_mut,
    str::from_utf8,
};

use aarch64_cpu::asm::barrier::{self, dsb};
use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
use alloc::alloc::alloc;
use klib::{
    acpi::{
        self, SystemDescription,
        fadt::Fadt,
        rsdp::{Rsdp, XsdtIter},
    },
    vm::{DMAP_START, MAIR_DEVICE_INDEX, MAIR_NORMAL_INDEX, TABLE_ENTRIES, TTENATIVE, TTable},
};
use log::{error, info};
use protocol::BootInfo;
use uefi::{
    CStr16, Status,
    allocator::Allocator,
    boot::{self, AllocateType, MemoryType, PAGE_SIZE as UEFI_PAGE_SIZE, memory_map},
    entry,
    mem::memory_map::MemoryMap,
    proto::media::file::{File, FileAttribute, FileInfo, FileMode},
    table::cfg::ConfigTableEntry,
};

use crate::page::{alloc_table, cpu_init, map_region, mmu_init, uefi_addr_to_paddr};

const PAGE_SIZE: usize = 16384;

#[global_allocator]
static ALLOC: Allocator = Allocator;

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

#[inline(always)]
fn busy_loop_ret() {
    loop {
        dsb(barrier::SY);
        unsafe { __wfe() };
    }
}

fn busy_loop_noret() -> ! {
    loop {
        dsb(barrier::SY);
        unsafe { __wfe() }
    }
}

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();

    cpu_init();
    info!("Loader starting...");
    let mut root_table = alloc_table();

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
            let arm_flags = fadt.arm_flags();
            info!("arm boot arch: {:#06x}", arm_flags);
            if (arm_flags & 1) != 0 {
                info!("PSCI compliant");
            }
        }

        if let Some(spcr) = sys.spcr {
            let base = spcr.base_addr.address();
            let type_ = spcr.interface_type();
            info!("SPCR serial type: {}, base: {:#x}", type_, base);
        }

        if let Some(madt) = sys.madt {
            info!("MADT found");

            for (t, data) in madt.entries() {
                match t {
                    acpi::madt::MADT_GICC => {
                        if data.len() >= mem::size_of::<acpi::madt::GicCpuInterface>() {
                            let gicc =
                                unsafe { &*(data.as_ptr() as *const acpi::madt::GicCpuInterface) };
                            let mpidr = unsafe { ptr::read_unaligned(&raw const gicc.mpidr) };
                            info!("     GICC CPU MPIDR={:#x}", mpidr);
                        }
                    }
                    acpi::madt::MADT_GICD => {
                        if data.len() >= mem::size_of::<acpi::madt::GicDistributor>() {
                            let gicd =
                                unsafe { &*(data.as_ptr() as *const acpi::madt::GicDistributor) };
                            let base = unsafe { ptr::read_unaligned(&raw const gicd.phys_base) };
                            info!("     GICD Distributor Base={:#x}", base);
                        }
                    }
                    acpi::madt::MADT_GICR => {
                        if data.len() >= mem::size_of::<acpi::madt::GicRedistributor>() {
                            let gicr =
                                unsafe { &*(data.as_ptr() as *const acpi::madt::GicRedistributor) };
                            let base = unsafe {
                                ptr::read_unaligned(&raw const gicr.discovery_range_base)
                            };
                            info!("     GICR Redistributor Base={:#x}", base);
                        }
                    }
                    acpi::madt::MADT_ITS => {
                        if data.len() >= mem::size_of::<acpi::madt::GicIts>() {
                            let its = unsafe { &*(data.as_ptr() as *const acpi::madt::GicIts) };
                            let id = unsafe { ptr::read_unaligned(&raw const its.translation_id) };
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
            let virt = unsafe { ptr::read_unaligned(&raw const gtdt.virt_el1_gsiv) };
            let phys = unsafe { ptr::read_unaligned(&raw const gtdt.ns_el1_gsiv) };
            info!("GTDT timer GSIVs Virt={}, Phys={}", virt, phys);
        }
    }

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

    let mut kernel = fh.into_regular_file().unwrap();

    let mut info_buf = [0u8; 512];
    let file_info: &FileInfo = match kernel.get_info(&mut info_buf) {
        Ok(f) => f,
        Err(e) => {
            error!("file info failed: {:?}", e);
            return Status::LOAD_ERROR;
        }
    };

    let file_size = file_info.file_size() as usize;
    info!("kernel.elf size = {}", file_size);

    let layout = Layout::from_size_align(file_size, PAGE_SIZE).unwrap();
    let ptr = unsafe { alloc(layout) };
    let nn_ptr = NonNull::new(ptr).expect("alloc FAIL");

    let elf_bytes: &mut [u8] = unsafe { from_raw_parts_mut(nn_ptr.as_ptr(), file_size) };

    if let Err(e) = kernel.set_position(0) {
        error!("set position failed: {:?}", e);
        return Status::LOAD_ERROR;
    }

    let mut total_read = 0usize;
    while total_read < file_size {
        match kernel.read(&mut elf_bytes[total_read..]) {
            Ok(0) => break,
            Ok(r) => total_read += r,
            Err(e) => {
                error!("read fail: {:?}", e);
                return Status::LOAD_ERROR;
            }
        }
    }

    if total_read != file_size {
        error!("read {} bytes but kernel is {}", total_read, file_size);
        return Status::LOAD_ERROR;
    }

    info!("Read kernel.elf into RAM.");

    if elf_bytes.len() < size_of::<Elf64Ehdr>() {
        error!("ELF too small");
        return Status::LOAD_ERROR;
    }

    let ehdr = unsafe { &*(elf_bytes.as_ptr() as *const Elf64Ehdr) };

    if &ehdr.e_ident[0..4] != b"\x7FELF" || ehdr.e_ident[4] != 2 || ehdr.e_ident[5] != 1 {
        error!("Kernel isn't a 64-bit little endian ELF!");
        return Status::LOAD_ERROR;
    }

    if ehdr.e_machine != 0xb7 {
        error!("ELF not ARM64!: {:#x}", ehdr.e_machine)
    }

    let phoff = ehdr.e_phoff as usize;
    let phentsize = ehdr.e_phentsize as usize;
    let phnum = ehdr.e_phnum as usize;

    info!("phoff={} phentsize={} phnum={}", phoff, phentsize, phnum);

    if phoff + phnum * phentsize > elf_bytes.len() {
        error!("ELF headers out of range!");
        return Status::LOAD_ERROR;
    }

    let mut min_vaddr = u64::MAX;
    let mut max_vaddr = 0u64;
    for i in 0..phnum {
        let ph = unsafe { &*(elf_bytes.as_ptr().add(phoff + i * phentsize) as *const Elf64Phdr) };
        if ph.p_type == PT_LOAD {
            if ph.p_vaddr < min_vaddr {
                min_vaddr = ph.p_vaddr;
            }

            let end = ph.p_vaddr.saturating_add(ph.p_memsz);
            if end > max_vaddr {
                max_vaddr = end;
            }
        }
    }

    if min_vaddr == u64::MAX {
        error!("No PT_LOAD segments!");
        return Status::LOAD_ERROR;
    }

    let load_span = max_vaddr - min_vaddr;
    info!("load span: {:#x} bytes", load_span);

    const UEFI_PS: u64 = UEFI_PAGE_SIZE as u64;

    let load_size = ((load_span + UEFI_PS - 1) / UEFI_PS) * UEFI_PS;
    let pages = (load_size / UEFI_PS) as usize;

    // allocate extra page(s) so rounding up is safe
    let extra = PAGE_SIZE / UEFI_PAGE_SIZE;

    let alloc_result = boot::allocate_pages(
        AllocateType::AnyPages,
        MemoryType::LOADER_DATA,
        pages + extra,
    );
    let alloc_ptr = match alloc_result {
        //Ok(ptr) => ((ptr.as_ptr() as u64) + (PAGE_SIZE as u64) - 1) & !(PAGE_SIZE as u64 - 1),
        Ok(ptr) => ptr.as_ptr() as u64,
        Err(e) => {
            error!("page allocation failed: {:?}", e);
            return Status::OUT_OF_RESOURCES;
        }
    };

    let base_phys = (alloc_ptr + (PAGE_SIZE as u64) - 1) & !(PAGE_SIZE as u64 - 1);

    info!(
        "allocated {} UEFI pages ({} bytes) at {:#x}",
        pages, load_size, base_phys
    );

    unsafe {
        let slice = core::slice::from_raw_parts_mut(base_phys as *mut u8, pages * UEFI_PAGE_SIZE);
        for i in slice {
            write_volatile(i, 0);
        }
    }

    //let mut p_vaddr = 0u64;

    for i in 0..phnum {
        let ph = unsafe { &*(elf_bytes.as_ptr().add(phoff + i * phentsize) as *const Elf64Phdr) };
        if ph.p_type != PT_LOAD {
            continue;
        }

        let file_off = ph.p_offset as usize;
        let filesz = ph.p_filesz as usize;
        let memsz = ph.p_memsz as usize;
        let vaddr = TTENATIVE::align_down(ph.p_vaddr);

        let pages = TTENATIVE::align_up(memsz as u64).div(PAGE_SIZE as u64);

        info!(
            "PT_LOAD vaddr={:#x} file_off={:#x} filesz={:#x} memsz={:#x}",
            vaddr, file_off, filesz, memsz
        );

        let flags = PhdrFlags::from_bits_truncate(ph.p_flags);

        let r = flags.contains(PhdrFlags::READ);
        let w = flags.contains(PhdrFlags::WRITE);
        let x = flags.contains(PhdrFlags::EXEC);

        info!(
            "Perm: {}{}{}",
            if r { 'R' } else { '-' },
            if w { 'W' } else { '-' },
            if x { 'X' } else { '-' }
        );

        if w && x {
            panic!("Kernel must be W^X (because I said so). Bad news for JIT fans...");
        }

        if file_off + filesz > elf_bytes.len() {
            error!("Segment file data out of bounds!");
            return Status::LOAD_ERROR;
        }

        let mut offset = (vaddr - min_vaddr) as usize;
        let dst = TTENATIVE::align_down(base_phys + offset as u64) as *mut u8;
        offset = dst as usize - base_phys as usize;
        let src =
            TTENATIVE::align_down(unsafe { elf_bytes.as_ptr().add(file_off) } as u64) as *mut u8;

        info!(
            "base_phys {:#x} rounded down to {:#x}",
            base_phys, dst as usize
        );

        info!(
            "COPYING segment: src={:#x} dst={:#x} filesz={}",
            src as u64, dst as u64, filesz
        );

        unsafe {
            if filesz > 0 {
                copy_nonoverlapping(src, dst, filesz);
            }

            if memsz > filesz {
                let tail = dst.add(filesz);
                for j in 0..(memsz - filesz) {
                    write_volatile(tail.add(j), 0);
                }
            }
        }

        info!(
            "MAPPING segment: phys={:#x} virt={:#x} pages={}",
            dst as u64, vaddr, pages
        );

        let ap = if w {
            AccessPermission::PrivilegedReadWrite
        } else {
            assert!(r && !w);
            AccessPermission::PrivilegedReadOnly
        };

        info!(
            "VADDR dst {:#x} -> PADDR dst {:#x}",
            dst as usize,
            uefi_addr_to_paddr(dst as usize)
        );

        if x {
            info!(
                "mapping code @ vaddr {:#x} paddr {:#x} offset {:#x}",
                vaddr,
                uefi_addr_to_paddr(dst as usize),
                offset
            )
        }

        map_region(
            unsafe { root_table.as_mut() },
            uefi_addr_to_paddr(dst as usize),
            vaddr as usize,
            pages as usize * PAGE_SIZE,
            ap,
            Shareability::InnerShareable,
            true,
            !x,
            MAIR_NORMAL_INDEX,
        );
    }

    let entry_vaddr = ehdr.e_entry;
    if entry_vaddr < min_vaddr || entry_vaddr >= max_vaddr {
        error!(
            "entrypoint {:#x} not in load span {:#x}..{:#x}",
            entry_vaddr, min_vaddr, max_vaddr
        );
        return Status::LOAD_ERROR;
    }

    let mem_map = boot::memory_map(MemoryType::LOADER_DATA).unwrap();

    for entry in mem_map.entries() {
        //info!("raw phys: {:#x}", entry.phys_start);
        let phys_start =
            TTENATIVE::align_down(uefi_addr_to_paddr(entry.phys_start as usize) as u64) as usize;
        let size = TTENATIVE::align_up(entry.page_count * UEFI_PS) as usize;
        let vaddr = TTENATIVE::align_down((DMAP_START + phys_start) as u64) as usize;

        //info!(
        //    "map type {:?} attr {:?} @ phys {:#x}, {} pages, virt {:#x}",
        //    entry.ty,
        //    entry.att,
        //    phys_start,
        //    size / UEFI_PS as usize,
        //    vaddr
        //);
        info!("{:?}", entry);

        match entry.ty {
            // RW no exec
            MemoryType::CONVENTIONAL
            | MemoryType::LOADER_DATA
            | MemoryType::BOOT_SERVICES_DATA
            | MemoryType::RUNTIME_SERVICES_DATA => {
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
                );
            }

            // RO exec
            MemoryType::LOADER_CODE
            | MemoryType::BOOT_SERVICES_CODE
            | MemoryType::RUNTIME_SERVICES_CODE => {
                map_region(
                    unsafe { root_table.as_mut() },
                    phys_start,
                    vaddr,
                    size,
                    AccessPermission::PrivilegedReadOnly,
                    Shareability::InnerShareable,
                    true,
                    false,
                    MAIR_NORMAL_INDEX,
                );
            }

            // RO no exec
            MemoryType::ACPI_RECLAIM => {
                map_region(
                    unsafe { root_table.as_mut() },
                    phys_start,
                    vaddr,
                    size,
                    AccessPermission::PrivilegedReadOnly,
                    Shareability::InnerShareable,
                    true,
                    true,
                    MAIR_NORMAL_INDEX,
                );
            }

            // RO no exec device
            MemoryType::ACPI_NON_VOLATILE => {
                map_region(
                    unsafe { root_table.as_mut() },
                    phys_start,
                    vaddr,
                    size,
                    AccessPermission::PrivilegedReadOnly,
                    Shareability::OuterShareable,
                    true,
                    true,
                    MAIR_DEVICE_INDEX,
                );
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
                );
            }

            _ => {
                info!("Unrecognized UEFI mem map entry type: {:?}", entry);
            }
        }
    }

    let entry_offset = (entry_vaddr - min_vaddr) as usize;
    let entry_addr = entry_vaddr as usize;

    mmu_init(root_table.as_ptr());

    info!(
        "entry at physical {:#x} virt {:#x} (offset {:#x})",
        base_phys + entry_offset as u64,
        entry_addr,
        entry_offset
    );

    let entry_fn: fn(boot_info_ptr: *mut BootInfo) -> ! = unsafe { transmute(entry_addr) };

    let mem_map_final = unsafe { boot::exit_boot_services(None) };

    entry_fn(&mut BootInfo {
        kernel_load_physical_address: base_phys as usize,
        kernel_size: load_size as usize,
        memory_map: mem_map_final,
    });
}
