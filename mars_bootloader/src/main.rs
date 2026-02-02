#![no_std]
#![no_main]

use core::time::Duration;

use log::{error, info};
use uefi::{
    CStr16, Status,
    boot::{self, get_image_file_system},
    entry,
    proto::media::{
        file::{File, FileAttribute, FileMode},
        fs::SimpleFileSystem,
    },
};

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const DT_NULL: i64 = 0;
const DT_RELA: i64 = 7;
const DT_RELASZ: i64 = 8;
const DT_RELAENT: i64 = 0;

const R_AARCH64_RELATIVE: u32 = 1027;

#[repr(C)]
#[derive(Clone, Copy)]
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
#[derive(Clone, Copy)]
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

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();

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

    boot::stall(Duration::from_secs(10));

    Status::SUCCESS
}
