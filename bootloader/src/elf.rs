use alloc::{vec, vec::Vec};
use core::ptr::{copy_nonoverlapping, read_unaligned, write_bytes};
use hal::paging::{Access, MappingOptions, MemoryType, valid_virtual_address};
use klib::{
    cache::{clean_dcache_range, clean_icache_range},
    vm::{PAGE_SIZE, align_down},
};
use uefi::{
    Status,
    proto::media::file::{File, FileInfo, RegularFile},
};

use crate::{Elf64Ehdr, Elf64Phdr, PT_LOAD, PhdrFlags, allocator::allocate_aligned_pages};

pub struct LoadedSegment {
    pub virtual_address: usize,
    pub physical_address: usize,
    pub size: usize,
    pub options: MappingOptions,
}

pub struct LoadedKernel {
    pub entry: usize,
    pub physical_address: usize,
    pub size: usize,
    pub segments: Vec<LoadedSegment>,
}

fn page_end(address: usize) -> Result<usize, Status> {
    address
        .checked_add(PAGE_SIZE - 1)
        .map(|end| align_down(end, PAGE_SIZE))
        .ok_or(Status::LOAD_ERROR)
}

pub fn load_kernel(mut kernel: RegularFile) -> Result<LoadedKernel, Status> {
    let mut info_buf = [0u8; 512];

    let file_info: &FileInfo = kernel
        .get_info(&mut info_buf)
        .map_err(|error| error.status())?;

    let file_size = usize::try_from(file_info.file_size()).map_err(|_| Status::LOAD_ERROR)?;
    let mut bytes = vec![0u8; file_size];

    kernel.set_position(0).map_err(|error| error.status())?;

    let mut total_read = 0;
    while total_read < bytes.len() {
        let count = kernel
            .read(&mut bytes[total_read..])
            .map_err(|error| error.status())?;
        if count == 0 {
            return Err(Status::LOAD_ERROR);
        }
        total_read += count;
    }

    if bytes.len() < size_of::<Elf64Ehdr>() {
        return Err(Status::LOAD_ERROR);
    }

    // file storage doesn't need to have the alignment of ELF structs
    let header = unsafe { read_unaligned(bytes.as_ptr().cast::<Elf64Ehdr>()) };
    if &header.e_ident[..4] != b"\x7fELF"
        || header.e_ident[4] != 2
        || header.e_ident[5] != 1
        || header.e_ident[6] != 1
        || header.e_type != 2
        || header.e_machine != 0xb7
        || header.e_version != 1
        || header.e_ehsize as usize != size_of::<Elf64Ehdr>()
        || header.e_phentsize as usize != size_of::<Elf64Phdr>()
    {
        return Err(Status::LOAD_ERROR);
    }

    let phoff = usize::try_from(header.e_phoff).map_err(|_| Status::LOAD_ERROR)?;
    let phnum = header.e_phnum as usize;
    let phbytes = phnum
        .checked_mul(size_of::<Elf64Phdr>())
        .ok_or(Status::LOAD_ERROR)?;

    if phoff.checked_add(phbytes).ok_or(Status::LOAD_ERROR)? > bytes.len() {
        return Err(Status::LOAD_ERROR);
    }

    let mut headers = Vec::new();
    let mut segments: Vec<LoadedSegment> = Vec::new();
    let mut minimum = usize::MAX;
    let mut maximum = 0;

    let entry = usize::try_from(header.e_entry).map_err(|_| Status::LOAD_ERROR)?;

    let mut executable_entry = false;

    for index in 0..phnum {
        let segment = unsafe {
            read_unaligned(
                bytes
                    .as_ptr()
                    .add(phoff + index * size_of::<Elf64Phdr>())
                    .cast::<Elf64Phdr>(),
            )
        };

        if segment.p_type != PT_LOAD {
            continue;
        }

        if segment.p_filesz > segment.p_memsz {
            return Err(Status::LOAD_ERROR);
        }

        let file_start = usize::try_from(segment.p_offset).map_err(|_| Status::LOAD_ERROR)?;
        let file_size = usize::try_from(segment.p_filesz).map_err(|_| Status::LOAD_ERROR)?;

        if file_start
            .checked_add(file_size)
            .ok_or(Status::LOAD_ERROR)?
            > bytes.len()
        {
            return Err(Status::LOAD_ERROR);
        }

        if segment.p_align > 1
            && (!segment.p_align.is_power_of_two()
                || segment.p_vaddr % segment.p_align != segment.p_offset % segment.p_align)
        {
            return Err(Status::LOAD_ERROR);
        }

        if segment.p_memsz == 0 {
            continue;
        }

        let start = usize::try_from(segment.p_vaddr).map_err(|_| Status::LOAD_ERROR)?;
        let size = usize::try_from(segment.p_memsz).map_err(|_| Status::LOAD_ERROR)?;
        let end = start.checked_add(size).ok_or(Status::LOAD_ERROR)?;

        // the bootstrap image belongs to the upper address half
        if start >> 48 != 0xffff || !valid_virtual_address(end - 1) {
            return Err(Status::LOAD_ERROR);
        }

        let flags = PhdrFlags::from_bits(segment.p_flags).ok_or(Status::LOAD_ERROR)?;
        if !flags.contains(PhdrFlags::READ) || flags.contains(PhdrFlags::WRITE | PhdrFlags::EXEC) {
            return Err(Status::LOAD_ERROR);
        }

        let aligned_start = align_down(start, PAGE_SIZE);
        let aligned_end = page_end(end)?;

        if segments.iter().any(|other| {
            aligned_start < other.virtual_address + other.size
                && other.virtual_address < aligned_end
        }) {
            return Err(Status::LOAD_ERROR);
        }

        let executable = flags.contains(PhdrFlags::EXEC);

        executable_entry |= executable && start <= entry && entry < end;
        minimum = minimum.min(aligned_start);
        maximum = maximum.max(aligned_end);

        segments.push(LoadedSegment {
            virtual_address: aligned_start,
            physical_address: 0,
            size: aligned_end - aligned_start,
            options: MappingOptions {
                access: if flags.contains(PhdrFlags::WRITE) {
                    Access::KernelReadWrite
                } else {
                    Access::KernelReadOnly
                },
                executable,
                memory_type: MemoryType::Normal,
            },
        });
        headers.push(segment);
    }

    if segments.is_empty() || !executable_entry {
        return Err(Status::LOAD_ERROR);
    }

    let size = maximum - minimum;
    let allocation = allocate_aligned_pages(size, PAGE_SIZE)?;
    let physical_address = allocation.as_ptr() as usize;

    unsafe { write_bytes(physical_address as *mut u8, 0, size) };

    for segment in headers {
        let offset = segment.p_vaddr as usize - minimum;
        unsafe {
            copy_nonoverlapping(
                bytes.as_ptr().add(segment.p_offset as usize),
                (physical_address + offset) as *mut u8,
                segment.p_filesz as usize,
            );
        }
    }

    for segment in &mut segments {
        segment.physical_address = physical_address + segment.virtual_address - minimum;
    }

    unsafe {
        clean_dcache_range(physical_address as *mut u8, size);
        clean_icache_range(physical_address as *mut u8, size);
    }

    Ok(LoadedKernel {
        entry,
        physical_address,
        size,
        segments,
    })
}
