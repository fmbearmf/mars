use core::ptr;

use klib::vm::user::PAGE_DESCRIPTORS;
use log::{LevelFilter, info, trace};
use protocol::BootInfo;
use uefi::mem::memory_map::{MemoryMap, MemoryMapMut};

use crate::{
    BOOT_INFO, KALLOCATOR, KERNEL_ADDRESS_SPACE,
    earlyinit::{
        acpi::acpi_init,
        earlycon::{EARLYCON, EarlyCon},
        mem::{
            clone_and_process_mmap, create_page_descriptors, populate_alloc_stage0,
            populate_alloc_stage1, switch_to_new_page_tables,
        },
        mmu::init_mmu,
    },
    log::LOGGER,
    print_mem_usage,
};

pub fn uefi_arm64_bootstrap(boot_info_ref: *mut BootInfo) {
    #[allow(static_mut_refs, reason = "singlethreaded access")]
    {
        unsafe {
            ptr::copy_nonoverlapping(boot_info_ref, BOOT_INFO.as_mut_ptr(), 1);
        };
    }

    #[allow(static_mut_refs, reason = "singlethreaded access")]
    let boot_info = unsafe { BOOT_INFO.assume_init_mut() };

    {
        let mut lock = EARLYCON.lock();
        *lock = Some(EarlyCon::new(boot_info.serial_uart_address));
    }

    LOGGER
        .init(LevelFilter::Trace)
        .expect("failed to init logger");

    info!("welcome to the jungle, we take it day by day");

    trace!("address of passed bootinfo ptr: {:#p}", boot_info_ref);
    trace!("address of bootinfo: {:#p}", &boot_info);

    trace!("init_mmu addr: {:#p}", init_mmu as *const ());
    init_mmu(boot_info.page_table_root);

    let uefi_mmap = &mut boot_info.memory_map;
    uefi_mmap.sort();

    trace!("uefi_mmap @ {:p}", uefi_mmap.buffer() as *const _);

    let uefi_mmap = clone_and_process_mmap(uefi_mmap);
    trace!("processed uefi_mmap @ {:p}", uefi_mmap.buffer() as *const _);

    for desc in uefi_mmap.entries() {
        trace!("{:x?}", desc);
    }

    populate_alloc_stage0(&uefi_mmap);

    let new_pt = unsafe { switch_to_new_page_tables(|| uefi_mmap.entries(), &KALLOCATOR) };

    unsafe { KALLOCATOR.transition_dmap() };

    let (page_descriptors, range) = create_page_descriptors();
    PAGE_DESCRIPTORS.init(page_descriptors, range);

    KERNEL_ADDRESS_SPACE.init_from_table(new_pt);

    print_mem_usage();

    acpi_init();

    populate_alloc_stage1(&uefi_mmap);

    print_mem_usage();

    trace!("dsfsdf");

    //print_pt(unsafe { pt_root.as_mut() }, false);
}
