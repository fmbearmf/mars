use core::ops::Range;

/// descriptor of a hardware resource
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    Mmio { range: Range<usize> },
    Irq { number: u32, polarity: IrqPolarity },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IrqPolarity {
    ActiveHigh,
    ActiveLow,
}
