use core::ops::Range;

/// descriptor of a hardware resource
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    Mmio {
        range: Range<usize>,
    },
    Irq(u32),
    PciEcam {
        segment: u16,
        bus: u8,
        device: u8,
        function: u8,
        ecam_phys_base: u64,
        ecam_start_bus: u8,
        ecam_end_bus: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IrqPolarity {
    ActiveHigh,
    ActiveLow,
}
