use tock_registers::{
    interfaces::{ReadWriteable, Readable, Writeable},
    register_bitfields, register_structs,
    registers::{ReadOnly, ReadWrite, WriteOnly},
};

pub mod gicv3;

pub trait InterruptController {
    type Error;

    /// initializes the controller
    fn init(&mut self) -> Result<(), Self::Error>;

    /// enables a specific interrupt by id
    fn enable_interrupt(&mut self, int_id: u32) -> Result<(), Self::Error>;

    /// disables a specific interrupt by id
    fn disable_interrupt(&mut self, int_id: u32) -> Result<(), Self::Error>;

    /// acknowledges highest priority pending interrupt
    /// `Some(id)` if the interrupt is pending; `None` if it's spurious.
    fn acknowledge_interrupt(&mut self) -> Result<Option<u32>, Self::Error>;

    /// signals EOI for an interrupt id
    fn end_of_interrupt(&mut self, int_id: u32) -> Result<(), Self::Error>;

    /// sets priority of interrupt
    fn set_priority(&mut self, int_id: u32, priority: u8) -> Result<(), Self::Error>;

    /// routes interrupt to a specific CPU
    fn set_affinity(&mut self, int_id: u32, affinity: u64) -> Result<(), Self::Error>;
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

register_bitfields! {
    u32,

    pub GICD_CTLR [
        ARE_NS OFFSET(4) NUMBITS(1)[ Enable = 1, Disable = 0 ],
        ENABLE_G1A OFFSET(1) NUMBITS(1)[ Enable = 1, Disable = 0 ],
        ENABLE_G1 OFFSET(0) NUMBITS(1)[ Enable = 1, Disable = 0 ],
    ],

    pub GICR_WAKER [
        ChildrenAsleep OFFSET(2) NUMBITS(1)[ True = 1, False = 0 ],
        ProcessorAsleep OFFSET(1) NUMBITS(1)[ Sleep = 1, Awake = 0 ],
    ],
}

register_structs! {
    #[allow(non_snake_case)]
    pub GicdRegisters {
        (0x0000 => pub CTLR: ReadWrite<u32, GICD_CTLR::Register>),
        (0x0004 => pub TYPER: ReadOnly<u32>),
        (0x0008 => pub IIDR: ReadOnly<u32>),
        (0x000C => _reserved0),
        (0x0100 => pub IGROUPR:[ReadWrite<u32>; 32]),
        (0x0180 => pub ISENABLER: [ReadWrite<u32>; 32]),
        (0x0200 => pub ICENABLER: [ReadWrite<u32>; 32]),
        (0x0280 => pub ISPENDR: [ReadWrite<u32>; 32]),
        (0x0300 => pub ICPENDR: [ReadWrite<u32>; 32]),
        (0x0380 => pub ISACTIVER:[ReadWrite<u32>; 32]),
        // GICv3 allows 8-bit access to priority registers
        // `u8` prevents race conditions across cores
        (0x0400 => pub IPRIORITYR: [ReadWrite<u8>; 1020]),
        (0x07FC => _reserved1),
        (0x0C00 => pub ICFGR:[ReadWrite<u32>; 64]),
        (0x0D00 => _reserved2),
        // IROUTER is present for SPIs (32-1019). so map 1024 elements starting at 0x6000
        // to align the array index with `int_id`
        (0x6000 => pub IROUTER:[ReadWrite<u64>; 1024]),
        (0x8000 => @END),
    }
}
register_structs! {
    #[allow(non_snake_case)]
    pub GicrRdRegisters {
        (0x0000 => pub CTLR: ReadWrite<u32>),
        (0x0004 => pub IIDR: ReadOnly<u32>),
        (0x0008 => pub TYPER: ReadOnly<u64>),
        (0x0010 => _reserved0),
        (0x0014 => pub WAKER: ReadWrite<u32, GICR_WAKER::Register>),
        (0x0018 => @END),
    }
}
register_structs! {
    #[allow(non_snake_case)]
    pub GicrSgiRegisters {
        (0x0000 => _reserved0),
        (0x0100 => pub IGROUPR0: ReadWrite<u32>),
        (0x0104 => _reserved1),
        (0x0180 => pub ISENABLER0: ReadWrite<u32>),
        (0x0184 => _reserved2),
        (0x0200 => pub ICENABLER0: ReadWrite<u32>),
        (0x0204 => _reserved3),
        (0x0280 => pub ISPENDR0: ReadWrite<u32>),
        (0x0284 => _reserved4),
        (0x0300 => pub ICPENDR0: ReadWrite<u32>),
        (0x0304 => _reserved5),
        (0x0380 => pub ISACTIVER0: ReadWrite<u32>),
        (0x0384 => _reserved6),
        (0x0400 => pub IPRIORITYR: [ReadWrite<u8>; 32]),
        (0x0420 => _reserved7),
        (0x0C00 => pub ICFGR0: ReadWrite<u32>),
        (0x0C04 => pub ICFGR1: ReadWrite<u32>),
        (0x0C08 => @END),
    }
}
