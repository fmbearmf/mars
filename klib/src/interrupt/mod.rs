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

use gicv3::registers::gic::{
    GicBitfield32, GicBitfield64, GicIcfgr, GicdCtlr, GicdTyper, GicrCtlr, GicrPropBar, GicrTyper,
    GicrWaker,
};

use crate::interrupt::gicv3::IrqHandler;
use crate::interrupt::gicv3::registers::gic::GicBitfield8;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum InterruptError {
    InvalidInterruptId,
    NotSupported,
    HandlerNotFound,
}

type Result<T> = core::result::Result<T, InterruptError>;

pub trait InterruptController: Send + Sync {
    /// initializes the controller.
    fn init(&self) -> Result<()>;

    /// enables a specific interrupt by id.
    fn enable_interrupt(&self, int_id: u32) -> Result<()>;

    /// disables a specific interrupt by id.
    fn disable_interrupt(&self, int_id: u32) -> Result<()>;

    /// acknowledges highest priority pending interrupt.
    /// `Some(id)` if the interrupt is pending; `None` if it's spurious.
    fn acknowledge_interrupt(&self) -> Result<Option<u32>>;

    /// signals EOI for an interrupt id.
    fn end_of_interrupt(&self, int_id: u32) -> Result<()>;

    /// sets priority of interrupt.
    fn set_priority(&self, int_id: u32, priority: u8) -> Result<()>;

    /// routes interrupt to a specific CPU.
    fn set_affinity(&self, int_id: u32, affinity: u64) -> Result<()>;

    /// handles an interrupt, as the name suggests.
    fn on_interrupt(&self, int_id: u32) -> Result<()>;

    /// register an interrupt hander
    fn register_handler(&self, int_id: u32, handler: IrqHandler) -> Result<()>;
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

declare_structs!(
    #[derive(KnownLayout, FromBytes, IntoBytes)]
    pub GicdRegisters {
        (0x0000 => pub ctl: RPureReadPureWrite<u32, GicdCtlr>),
        (0x0004 => pub type_: RPureReadOnly<u32, GicdTyper>),
        (0x0080 => pub igroup: [RPureReadPureWrite<u32, GicBitfield32>; 32]),
        (0x0100 => pub iset_enable: [RPureReadPureWrite<u32, GicBitfield32>; 32]),
        (0x0180 => pub iclear_enable: [RPureReadPureWrite<u32, GicBitfield32>; 32]),
        (0x0200 => pub iset_pend: [RPureReadPureWrite<u32, GicBitfield32>; 32]),
        (0x0280 => pub iclear_pend: [RPureReadPureWrite<u32, GicBitfield32>; 32]),
        (0x0300 => pub iset_active: [RPureReadPureWrite<u32, GicBitfield32>; 32]),
        (0x0380 => pub iclear_active: [RPureReadPureWrite<u32, GicBitfield32>; 32]),
        (0x0400 => pub ipriority: [RPureReadPureWrite<u8, GicBitfield8>; 1024]),
        (0x0C00 => pub icfg: [RPureReadPureWrite<u32, GicIcfgr>; 64]),
        (0x6100 => pub irouter: [RPureReadPureWrite<u64, GicBitfield64>; 992]),
        (0x10000 => @END)
    }
);

unsafe impl Sync for GicdRegisters {}

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
        (0x10400 => ipriority: [RPureReadWrite<u8, GicBitfield8>; 32]),
        (0x10C00 => icfg0: RPureReadWrite<u32, GicIcfgr>),
        (0x10C04 => icfg1: RPureReadWrite<u32, GicIcfgr>),
        (0x10D00 => igroup_mod: RPureReadWrite<u32, GicBitfield32>),
        (0x20000 => @END)
    }
);
