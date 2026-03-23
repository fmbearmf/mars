use aarch64_cpu::{
    asm::{self},
    registers::{MAIR_EL1, SCTLR_EL1, TCR_EL1},
};
use tock_registers::interfaces::*;

unsafe extern "C" {
    static __KEND: usize;
}

//#[unsafe(link_section = ".reclaimable.bss")]
//static mut PT_POOL: [TTable<TABLE_ENTRIES>; MAX_TABLES] = [TTable::new(); MAX_TABLES];

pub extern "C" fn init_mmu(_load_addr: usize, _offset: usize) {
    // attr0=device, attr1=normal
    MAIR_EL1.write(
        MAIR_EL1::Attr0_Device::nonGathering_nonReordering_EarlyWriteAck
            + MAIR_EL1::Attr1_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc
            + MAIR_EL1::Attr1_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc,
    );

    TCR_EL1.modify(
        TCR_EL1::TBI1::Ignored
            + TCR_EL1::IPS::Bits_48
            + TCR_EL1::TG1::KiB_16
            + TCR_EL1::SH1::Inner
            + TCR_EL1::ORGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::IRGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::EPD1::EnableTTBR1Walks
            + TCR_EL1::T1SZ.val(16),
    );

    //TTBR0_EL1.set(LOW_L0 as *const _ as u64);
    //TTBR1_EL1.set(HIGH_L0 as *const _ as u64);
    SCTLR_EL1.modify(SCTLR_EL1::M::Enable + SCTLR_EL1::C::Cacheable + SCTLR_EL1::I::Cacheable);

    asm::barrier::isb(asm::barrier::SY);
    asm::barrier::dsb(asm::barrier::SY);
    asm::barrier::dsb(asm::barrier::ISH);
}
