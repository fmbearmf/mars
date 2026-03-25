use core::{
    alloc::Layout,
    ops::Div,
    ptr::{NonNull, copy_nonoverlapping, write_volatile},
    slice::from_raw_parts_mut,
};

use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
use alloc::alloc::alloc;
use klib::{
    vec::DynVec,
    vm::{
        MAIR_NORMAL_INDEX, MemoryRegion, MemoryRegionType, TABLE_ENTRIES, TTENATIVE, TTable,
        align_up, map::map_region,
    },
};
use log::{debug, error};
use uefi::{
    Status,
    boot::{self, AllocateType, MemoryType, PAGE_SIZE as UEFI_PAGE_SIZE},
    proto::media::file::{File, FileInfo, RegularFile},
};

use crate::{
    Elf64Ehdr, Elf64Phdr, PAGE_SIZE, PT_ALLOCATOR, PT_LOAD, PhdrFlags, UEFI_PS, vec::UefiVec,
};

pub fn load_kernel(
    mut kernel: RegularFile,
    mut root_table: NonNull<TTable<TABLE_ENTRIES>>,
    kregions: &mut UefiVec<MemoryRegion>,
) -> Result<(u64, u64, u64), Status> {
    let mut info_buf = [0u8; 512];
    let file_info: &FileInfo = match kernel.get_info(&mut info_buf) {
        Ok(f) => f,
        Err(e) => {
            error!("file info failed: {:?}", e);
            return Err(e.status());
        }
    };

    let file_size = file_info.file_size() as usize;
    debug!("kernel.elf size = {}", file_size);

    let layout = Layout::from_size_align(file_size, PAGE_SIZE).unwrap();
    let ptr = unsafe { alloc(layout) };
    let nn_ptr = NonNull::new(ptr).expect("alloc FAIL");

    let elf_bytes: &mut [u8] = unsafe { from_raw_parts_mut(nn_ptr.as_ptr(), file_size) };

    match kernel.set_position(0) {
        Err(e) => {
            error!("failed to set file position");
            return Err(e.status());
        }
        Ok(_) => {}
    };

    let mut total_read = 0usize;
    while total_read < file_size {
        match kernel.read(&mut elf_bytes[total_read..]) {
            Ok(0) => break,
            Ok(r) => total_read += r,
            Err(e) => {
                error!("read fail: {:?}", e);
                return Err(e.status());
            }
        }
    }

    if total_read != file_size {
        error!("read {} bytes but kernel is {}", total_read, file_size);
        return Err(Status::LOAD_ERROR);
    }

    debug!("Read kernel.elf into RAM.");

    if elf_bytes.len() < size_of::<Elf64Ehdr>() {
        error!("ELF too small");
        return Err(Status::LOAD_ERROR);
    }

    let ehdr = unsafe { &*(elf_bytes.as_ptr() as *const Elf64Ehdr) };

    if &ehdr.e_ident[0..4] != b"\x7FELF" || ehdr.e_ident[4] != 2 || ehdr.e_ident[5] != 1 {
        error!("Kernel isn't a 64-bit little endian ELF!");
        return Err(Status::LOAD_ERROR);
    }

    if ehdr.e_machine != 0xb7 {
        error!("ELF not ARM64!: {:#x}", ehdr.e_machine)
    }

    let phoff = ehdr.e_phoff as usize;
    let phentsize = ehdr.e_phentsize as usize;
    let phnum = ehdr.e_phnum as usize;

    debug!("phoff={} phentsize={} phnum={}", phoff, phentsize, phnum);

    if phoff + phnum * phentsize > elf_bytes.len() {
        error!("ELF headers out of range!");
        return Err(Status::LOAD_ERROR);
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
        return Err(Status::LOAD_ERROR);
    }

    let load_span = max_vaddr - min_vaddr;
    debug!("load span: {:#x} bytes", load_span);

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
        Ok(ptr) => ptr.as_ptr() as u64,
        Err(e) => {
            error!("page allocation failed: {:?}", e);
            return Err(e.status());
        }
    };

    let base_phys = align_up(alloc_ptr as usize, PAGE_SIZE) as u64;

    debug!(
        "allocated {} UEFI pages ({} bytes) at {:#x}",
        pages, load_size, base_phys
    );

    unsafe {
        let slice = core::slice::from_raw_parts_mut(base_phys as *mut u8, pages * UEFI_PAGE_SIZE);
        for i in slice {
            write_volatile(i, 0);
        }
    }

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

        debug!(
            "PT_LOAD vaddr={:#x} file_off={:#x} filesz={:#x} memsz={:#x}",
            vaddr, file_off, filesz, memsz
        );

        let flags = PhdrFlags::from_bits_truncate(ph.p_flags);

        let r = flags.contains(PhdrFlags::READ);
        let w = flags.contains(PhdrFlags::WRITE);
        let x = flags.contains(PhdrFlags::EXEC);

        debug!(
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
            return Err(Status::LOAD_ERROR);
        }

        let mut offset = (vaddr - min_vaddr) as usize;
        let dst = TTENATIVE::align_down(base_phys + offset as u64) as *mut u8;
        offset = dst as usize - base_phys as usize;
        let src =
            TTENATIVE::align_down(unsafe { elf_bytes.as_ptr().add(file_off) } as u64) as *mut u8;

        debug!(
            "base_phys {:#x} rounded down to {:#x}",
            base_phys, dst as usize
        );

        debug!(
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

        debug!(
            "MAPPING segment: phys={:#x} virt={:#x} pages={}",
            dst as u64, vaddr, pages
        );

        let ap = if w {
            AccessPermission::PrivilegedReadWrite
        } else {
            assert!(r && !w);
            AccessPermission::PrivilegedReadOnly
        };

        debug!(
            "VADDR dst {:#x} -> PADDR dst {:#x}",
            dst as usize,
            PT_ALLOCATOR.vaddr_to_paddr_uefi(dst as usize)
        );

        if x {
            debug!(
                "mapping code @ vaddr {:#x} paddr {:#x} offset {:#x}",
                vaddr,
                PT_ALLOCATOR.vaddr_to_paddr_uefi(dst as usize),
                offset
            )
        }

        map_region(
            unsafe { root_table.as_mut() },
            PT_ALLOCATOR.vaddr_to_paddr_uefi(dst as usize),
            vaddr as usize,
            pages as usize * PAGE_SIZE,
            ap,
            Shareability::InnerShareable,
            true,
            !x,
            MAIR_NORMAL_INDEX,
            &PT_ALLOCATOR,
        );

        let region_type = if r && x {
            MemoryRegionType::KernelCode
        } else if r && w {
            MemoryRegionType::KernelRwData
        } else {
            MemoryRegionType::KernelRoData
        };

        kregions.push(MemoryRegion {
            base: PT_ALLOCATOR.vaddr_to_paddr_uefi(dst as usize),
            size: pages as usize * PAGE_SIZE,
            region_type,
        });
    }

    let entry_vaddr = ehdr.e_entry;
    if entry_vaddr < min_vaddr || entry_vaddr >= max_vaddr {
        error!(
            "entrypoint {:#x} not in load span {:#x}..{:#x}",
            entry_vaddr, min_vaddr, max_vaddr
        );
        return Err(Status::LOAD_ERROR);
    }

    let entry_offset = entry_vaddr - min_vaddr;

    debug!(
        "entrypoint at physical {:#x} virt {:#x} (offset {:#x})",
        base_phys + entry_offset as u64,
        entry_vaddr,
        entry_offset
    );

    Ok((entry_vaddr, base_phys, load_size))
}
