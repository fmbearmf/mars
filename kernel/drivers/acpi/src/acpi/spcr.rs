use crate::acpi::AcpiTableTrait;
use crate::impl_table;

use super::FromBytes;
use super::{GenericAddress, header::SdtHeader};
use hax_lib::{attributes, ensures, opaque, requires};

impl_table! {
    #[derive(Debug, Clone, Copy)]
    pub struct Spcr {
        pub header: SdtHeader,
        pub interface_type: u8,
        pub reserved: [u8; 3],
        pub base_addr: GenericAddress,
        pub interrupt_type: u8,
        pub irq: u8,
        pub global_system_interrupt: u32,
        pub baud_rate: u8,
        pub parity: u8,
        pub stop_bits: u8,
        pub flow_control: u8,
        pub terminal_type: u8,
        pub reserved2: u8,
        pub pci_device_id: u16,
        pub pci_vendor_id: u16,
        pub pci_bus: u8,
        pub pci_device: u8,
        pub pci_function: u8,
        pub pci_flags: u32,
        pub pci_segment: u8,
        pub reserved3: u32,
    }
}

#[attributes]
impl AcpiTableTrait for Spcr {
    #[opaque]
    #[requires(slice.len() as usize >= core::mem::size_of::<Self>())]
    #[ensures(|result| result.is_ok())]
    fn safe_table_cast(slice: &'static [u8]) -> Result<&'static Self, &'static str> {
        let (reference, _) = Self::ref_from_prefix(slice).map_err(|_| "alignment/size error")?;
        Ok(reference)
    }
}
