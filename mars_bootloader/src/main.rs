#![no_std]
#![no_main]
#![feature(stdarch_arm_hints)]

extern crate alloc;

mod page;

use core::{
    alloc::Layout,
    arch::{aarch64::__wfe, asm},
    mem::{MaybeUninit, transmute},
    ptr::{self, NonNull, copy_nonoverlapping, write_bytes, write_volatile},
    slice::from_raw_parts_mut,
    time::Duration,
};

use alloc::{alloc::alloc, vec};
use alloc::{boxed::Box, vec::Vec};
use log::{error, info};
use mars_protocol::BootInfo;
use uefi::{
    CStr16, Status,
    allocator::Allocator,
    boot::{self, AllocateType, MemoryType, PAGE_SIZE as UEFI_PAGE_SIZE, get_image_file_system},
    entry,
    mem::{
        AlignedBuffer,
        memory_map::{MemoryMap, MemoryMapMut, MemoryMapOwned},
    },
    proto::media::file::{File, FileAttribute, FileInfo, FileMode},
};

use crate::page::mmu_init;

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

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Dyn {
    d_tag: i64,
    d_un: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Rela {
    r_offset: u64,
    r_info: u64,
    r_addend: i64,
}

#[inline(always)]
fn busy_loop_ret() {
    loop {
        unsafe { __wfe() };
    }
}

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();

    info!("Loader starting...");

    busy_loop_ret();

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

    let mut elf_bytes: &mut [u8] = unsafe { from_raw_parts_mut(nn_ptr.as_ptr(), file_size) };

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
        MemoryType::LOADER_CODE,
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

    mmu_init();

    for i in 0..phnum {
        let ph = unsafe { &*(elf_bytes.as_ptr().add(phoff + i * phentsize) as *const Elf64Phdr) };
        if ph.p_type != PT_LOAD {
            continue;
        }

        let file_off = ph.p_offset as usize;
        let filesz = ph.p_filesz as usize;
        let memsz = ph.p_memsz as usize;
        let vaddr = ph.p_vaddr;

        info!(
            "PT_LOAD vaddr={:#x} file_off={:#x} filesz={:#x} memsz={:#x}",
            vaddr, file_off, filesz, memsz
        );

        if file_off + filesz > elf_bytes.len() {
            error!("Segment file data out of bounds!");
            return Status::LOAD_ERROR;
        }

        let offset = (vaddr - min_vaddr) as usize;
        let dst = (base_phys as usize + offset) as *mut u8;
        let src = unsafe { elf_bytes.as_ptr().add(file_off) };

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
    }

    let entry_vaddr = ehdr.e_entry;
    if entry_vaddr < min_vaddr || entry_vaddr >= max_vaddr {
        error!(
            "entrypoint {:#x} not in load span {:#x}..{:#x}",
            entry_vaddr, min_vaddr, max_vaddr
        );
        return Status::LOAD_ERROR;
    }

    let entry_offset = (entry_vaddr - min_vaddr) as usize;
    let entry_addr = base_phys + entry_offset as u64;

    info!(
        "entry at physical {:#x} (offset {:#x})",
        entry_addr, entry_offset
    );

    let entry_fn: fn(boot_info_ptr: *mut BootInfo) -> ! = unsafe { transmute(entry_addr) };
    let mut mem_map = unsafe { boot::exit_boot_services(None) };

    entry_fn(&mut BootInfo {
        kernel_load_physical_address: base_phys as usize,
        kernel_size: load_size as usize,
        memory_map: mem_map,
    });
}
