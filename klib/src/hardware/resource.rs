use core::ops::Range;

/// descriptor of a hardware resource
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    Mmio {
        range: Range<usize>,
    },
    Irq(u32),
    /// Original PCI BAR register index, including holes and 64-bit upper halves.
    /// PCI discovery also emits `Mmio` for consumers that do not need BAR identity.
    PciBar {
        index: u8,
        range: Range<usize>,
    },
    /// Firmware-reported topology, not an established or usable DMA mapping.
    PciDmaTopology {
        address_bits: u8,
        coherent: bool,
        remapping: DmaRemapping,
    },
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

/// An IORT route describes the hardware that must be configured before DMA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaRemapping {
    /// No IOMMU on the reported route. PCI DMA windows still need resolving.
    NoIommu,
    ArmSmmu {
        base: u64,
        span: u64,
        model: u32,
        flags: u32,
        stream_id: u32,
    },
    ArmSmmuV3 {
        base: u64,
        flags: u32,
        model: u32,
        stream_id: u32,
    },
}

/// Resolve a PCI BAR register number, never a compacted MMIO-resource ordinal.
pub fn pci_bar_range(resources: &[Resource], bar_index: u8) -> Option<&Range<usize>> {
    resources.iter().find_map(|resource| match resource {
        Resource::PciBar { index, range } if *index == bar_index => Some(range),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_indices_preserve_64_bit_upper_halves_and_holes() {
        let resources = [
            Resource::PciBar {
                index: 0,
                range: 0x1000..0x2000,
            },
            Resource::Mmio {
                range: 0x1000..0x2000,
            },
            Resource::PciBar {
                index: 4,
                range: 0x8000..0x9000,
            },
            Resource::Mmio {
                range: 0x8000..0x9000,
            },
        ];
        assert_eq!(pci_bar_range(&resources, 0), Some(&(0x1000..0x2000)));
        assert_eq!(pci_bar_range(&resources, 4), Some(&(0x8000..0x9000)));
        for index in [1, 2, 3, 5, 6, u8::MAX] {
            assert_eq!(pci_bar_range(&resources, index), None);
        }
    }

    #[test]
    fn generic_mmio_does_not_establish_bar_identity() {
        let resources = [Resource::Mmio {
            range: 0x1000..0x2000,
        }];
        assert_eq!(pci_bar_range(&resources, 0), None);
        assert_eq!(pci_bar_range(&[], 0), None);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IrqPolarity {
    ActiveHigh,
    ActiveLow,
}
