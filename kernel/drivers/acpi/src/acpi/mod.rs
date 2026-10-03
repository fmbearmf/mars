pub mod fadt;
pub mod gtdt;
pub mod header;
pub mod iort;
pub mod madt;
pub mod mcfg;
pub mod spcr;
pub mod xsdp;

use alloc::boxed::Box;
use hax_lib::{attributes, ensures, exclude, opaque, requires};

use header::SdtHeader;
use spcr::Spcr;
use xsdp::{Xsdp, XsdtIter};

/// locate and validate the SPCR PL011 UART base address
/// safety: `xsdp_addr` and all physical ACPI table addresses must be readable as though identity mapped
pub unsafe fn discover_pl011_uart(xsdp_addr: usize) -> Result<usize, &'static str> {
    if xsdp_addr == 0 {
        return Err("missing ACPI RSDP");
    }

    let xsdp = Xsdp::try_from_addr(xsdp_addr)?;
    let xsdt = xsdp.xsdt(|addr| addr)?;
    let tables = XsdtIter::new(xsdt, Box::new(|addr| addr));
    for bytes in tables {
        if bytes.len() < 4 || &bytes[..4] != b"SPCR" {
            continue;
        }

        let table = Spcr::safe_table_cast(bytes)?;
        if table.interface_type() != 0x03 {
            log::info!("interface type: {:#x?}", table.interface_type());
            return Err("unsupported SPCR serial interface (expected ARM PL011)");
        }

        let gas = table.base_addr();
        if gas.address_space_id() != 0 || gas.register_bit_width() == 0 {
            return Err("unsupported SPCR UART memory GAS");
        }

        let address = usize::try_from(gas.address())
            .map_err(|_| "SPCR UART address exceeds physical address width")?;

        address
            .checked_add(0x1000)
            .ok_or("SPCR UART range overflows")?;

        return Ok(address);
    }
    Err("ACPI SPCR table not found")
}

#[exclude]
pub(self) use zerocopy;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

#[macro_export]
macro_rules! impl_table {
    ($(#[$meta:meta])* $vis:vis struct $name:ident { $($field_vis:vis $field_name:ident : $field_type:ty),* $(,)? }) => {
        #[cfg(not(hax))]
        #[derive(crate::acpi::zerocopy::FromBytes, crate::acpi::zerocopy::IntoBytes, crate::acpi::zerocopy::KnownLayout, crate::acpi::zerocopy::Immutable, crate::acpi::zerocopy::Unaligned)]
        $(#[$meta])*
        #[repr(C, packed)]
        #[mars_getters::unaligned_getters]
        $vis struct $name {
            $($field_vis $field_name : $field_type),*
        }

        #[cfg(hax)]
        $(#[$meta])*
        #[derive(crate::acpi::zerocopy::FromBytes, crate::acpi::zerocopy::IntoBytes, crate::acpi::zerocopy::KnownLayout, crate::acpi::zerocopy::Immutable, crate::acpi::zerocopy::Unaligned)]
        #[repr(C, packed)]
        #[mars_getters::unaligned_getters_hax]
        $vis struct $name {
            $($field_vis $field_name : $field_type),*
        }

        //#[cfg(hax)]
        //#[hax_lib::opaque]
        //impl crate::acpi::zerocopy_shim::KnownLayout for $name {
        //    fn kl_noop() {
        //        unimplemented!();
        //    }
        //}
        //#[cfg(hax)]
        //#[hax_lib::opaque]
        //impl crate::acpi::zerocopy_shim::Immutable for $name {
        //    fn im_noop() {
        //        unimplemented!();
        //    }
        //}
        //#[cfg(hax)]
        //#[hax_lib::opaque]
        //impl crate::acpi::zerocopy_shim::Unaligned for $name {
        //    fn ul_noop() {
        //        unimplemented!();
        //    }
        //}
    };
}

#[attributes]
pub trait AcpiTableTrait: FromBytes + KnownLayout + Unaligned + Immutable {
    #[ensures(|result| result.is_ok())]
    #[requires(slice.len() as usize >= core::mem::size_of::<Self>())]
    fn safe_table_cast(slice: &'static [u8]) -> Result<&'static Self, &'static str>;
}

impl_table! {
#[derive(Debug, Clone, Copy)]
    pub struct GenericAddress {
        pub address_space_id: u8,
        pub register_bit_width: u8,
        pub register_bit_offset: u8,
        pub access_size: u8,
        pub address: u64,
    }
}

pub fn checksum(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
}

#[opaque]
#[requires(slice.len() as usize >= core::mem::size_of::<T>())]
#[ensures(|result| result.is_ok())]
fn safe_table_cast<T: AcpiTableTrait + FromBytes + KnownLayout + Unaligned + Immutable>(
    slice: &'static [u8],
) -> Result<&'static T, &'static str> {
    let (reference, _) = match T::ref_from_prefix(slice) {
        Ok(re) => re,
        Err(_) => unreachable!(),
    };
    Ok(reference)
}

#[opaque]
#[ensures(|result| result.len() as usize == len)]
fn get_memory_slice(addr: usize, len: usize) -> &'static [u8] {
    unsafe { core::slice::from_raw_parts(addr as *const u8, len) }
}

#[opaque]
fn get_ref_addr<T>(r: &T) -> usize {
    r as *const T as usize
}
