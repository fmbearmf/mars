use core::ops::Range;

/// descriptor of a hardware resource
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    Mmio { range: Range<usize> },
    Irq(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IrqPolarity {
    ActiveHigh,
    ActiveLow,
}
