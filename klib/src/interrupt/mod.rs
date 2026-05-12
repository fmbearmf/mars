use mars_models::memory::registers::volatile::{
    PureReadable, RPureReadOnly, RPureReadPureWrite, RPureReadWrite, RWriteOnly, Writeable,
};

use mars_models::declare_structs;
use tock_registers::{
    register_structs,
    registers::{ReadOnly, ReadWrite},
};
use zerocopy::*;

pub mod gicv3;

use gicv3::registers::{
    GICD_CTLR, GICD_ICFGR, GICD_IIDR, GICD_INT, GICD_IROUTER, GICD_TYPER, GICR_CTLR, GICR_WAKER,
    gic::{
        GicBitfield32, GicBitfield64, GicIcfgr, GicdCtlr, GicdTyper, GicrCtlr, GicrPropBar,
        GicrTyper, GicrWaker,
    },
};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum InterruptError {
    InvalidInterruptId,
    NotSupported,
}

pub trait InterruptController: Send + Sync {
    /// initializes the controller.
    fn init(&self) -> Result<(), InterruptError>;

    /// enables a specific interrupt by id.
    fn enable_interrupt(&self, int_id: u32) -> Result<(), InterruptError>;

    /// disables a specific interrupt by id.
    fn disable_interrupt(&self, int_id: u32) -> Result<(), InterruptError>;

    /// acknowledges highest priority pending interrupt.
    /// `Some(id)` if the interrupt is pending; `None` if it's spurious.
    fn acknowledge_interrupt(&self) -> Result<Option<u32>, InterruptError>;

    /// signals EOI for an interrupt id.
    fn end_of_interrupt(&self, int_id: u32) -> Result<(), InterruptError>;

    /// sets priority of interrupt.
    fn set_priority(&self, int_id: u32, priority: u8) -> Result<(), InterruptError>;

    /// routes interrupt to a specific CPU.
    fn set_affinity(&self, int_id: u32, affinity: u64) -> Result<(), InterruptError>;
}

/// abstract interface
pub trait InterruptInterface {
    /// read interrupt ack register
    fn read_iar(&self) -> u32;

    /// write end of interrupt (EOI) register
    fn write_eoir(&self, int_id: u32);

    /// enable group 1 interrupts
    fn enable_group1(&self);

    /// disable group 1 interrupts
    fn disable_group1(&self);

    /// set priority mask
    fn set_priority_mask(&self, mask: u8);
}

register_structs! {
    #[allow(non_snake_case)]
    #[derive(KnownLayout)]
    pub GicdRegisters {
        (0x0000 => pub CTLR: ReadWrite<u32, GICD_CTLR::Register>),
        (0x0004 => pub TYPER: ReadOnly<u32, GICD_TYPER::Register>),
        (0x0008 => pub IIDR: ReadOnly<u32, GICD_IIDR::Register>),
        (0x000C => _reserved0),
        (0x0100 => pub IGROUPR: [ReadWrite<u32, GICD_INT::Register>; 32]),
        (0x0180 => pub ISENABLER: [ReadWrite<u32, GICD_INT::Register>; 32]),
        (0x0200 => pub ICENABLER: [ReadWrite<u32, GICD_INT::Register>; 32]),
        (0x0280 => pub ISPENDR: [ReadWrite<u32, GICD_INT::Register>; 32]),
        (0x0300 => pub ICPENDR: [ReadWrite<u32, GICD_INT::Register>; 32]),
        (0x0380 => pub ISACTIVER: [ReadWrite<u32, GICD_INT::Register>; 32]),
        (0x0400 => pub IPRIORITYR: [ReadWrite<u8>; 1020]),
        (0x07FC => _reserved1),
        (0x0C00 => pub ICFGR: [ReadWrite<u32, GICD_ICFGR::Register>; 64]),
        (0x0D00 => _reserved2),
        // IROUTER is present for SPIs (32-1019). so map 1024 elements starting at 0x6000
        // to align the array index with `int_id`
        (0x6000 => pub IROUTER: [ReadWrite<u64, GICD_IROUTER::Register>; 1024]),
        (0x8000 => @END),
    }
}

declare_structs!(
    #[derive(KnownLayout, FromBytes, IntoBytes)]
    pub GicdRegistersN {
        (0x0000 => pub ctl: RPureReadPureWrite<u32, GicdCtlr>),
        (0x0004 => pub type_: RPureReadOnly<u32, GicdTyper>),
        (0x0080 => pub igroup: [RPureReadPureWrite<u32, GicBitfield32>; 32]),
        (0x0100 => pub iset_enable: [RPureReadPureWrite<u32, GicBitfield32>; 32]),
        (0x0180 => pub iclear_enable: [RPureReadPureWrite<u32, GicBitfield32>; 32]),
        (0x0200 => pub iset_pend: [RPureReadPureWrite<u32, GicBitfield32>; 32]),
        (0x0280 => pub iclear_pend: [RPureReadPureWrite<u32, GicBitfield32>; 32]),
        (0x0300 => pub iset_active: [RPureReadPureWrite<u32, GicBitfield32>; 32]),
        (0x0380 => pub iclear_active: [RPureReadPureWrite<u32, GicBitfield32>; 32]),
        (0x0400 => pub ipriority: [RPureReadPureWrite<u32, GicBitfield32>; 32]),
        (0x0C00 => pub icfg: [RPureReadPureWrite<u32, GicIcfgr>; 64]),
        (0x6100 => pub irouter: [RPureReadPureWrite<u64, GicBitfield64>; 992]),
        (0x10000 => @END)
    }
);

unsafe impl Sync for GicdRegistersN {}

declare_structs!(
    #[derive(KnownLayout, Immutable, FromBytes, IntoBytes)]
    pub GicrRegisters {
        // RD_base
        (0x0000 => pub ctl: RPureReadWrite<u32, GicrCtlr>),
        (0x0008 => pub type_: RPureReadOnly<u64, GicrTyper>),
        (0x0014 => pub wake: RPureReadWrite<u32, GicrWaker>),
        (0x0040 => pub set_lpi: RWriteOnly<u64, GicBitfield64>),
        (0x0048 => pub clear_lpi: RWriteOnly<u64, GicBitfield64>),
        (0x0070 => pub property_bar: RPureReadWrite<u64, GicrPropBar>),
        (0x0078 => pub pending_bar: RPureReadWrite<u64, GicBitfield64>),

        // SGI_base
        (0x10080 => igroup0: RPureReadWrite<u32, GicBitfield32>),
        (0x10100 => iset_enable0: RPureReadWrite<u32, GicBitfield32>),
        (0x10180 => iclear_enable0: RPureReadWrite<u32, GicBitfield32>),
        (0x10200 => iset_pend0: RPureReadWrite<u32, GicBitfield32>),
        (0x10280 => iclear_pend0: RPureReadWrite<u32, GicBitfield32>),
        (0x10300 => iset_active0: RPureReadWrite<u32, GicBitfield32>),
        (0x10380 => iclear_active0: RPureReadWrite<u32, GicBitfield32>),
        (0x10400 => ipriority: [RPureReadWrite<u32, GicBitfield32>; 8]),
        (0x10C00 => icfg0: RPureReadWrite<u32, GicIcfgr>),
        (0x10C04 => icfg1: RPureReadWrite<u32, GicIcfgr>),
        (0x10D00 => igroup_mod: RPureReadWrite<u32, GicBitfield32>),
        (0x20000 => @END)
    }
);
