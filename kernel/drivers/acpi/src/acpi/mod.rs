pub mod fadt;
pub mod gtdt;
pub mod header;
pub mod madt;
pub mod mcfg;
pub mod spcr;
pub mod xsdp;

use hax_lib::{attributes, ensures, exclude, opaque, requires};
use klib::hardware::device::DeviceTree;

use fadt::Fadt;
use gtdt::Gtdt;
use header::SdtHeader;
use madt::Madt;
use mcfg::Mcfg;
use spcr::Spcr;
use xsdp::XsdtIter;

#[exclude]
pub(self) use zerocopy;
use zerocopy::{FromBytes, Immutable, KnownLayout, Unaligned};

#[macro_export]
macro_rules! impl_table {
    ($(#[$meta:meta])* $vis:vis struct $name:ident { $($field_vis:vis $field_name:ident : $field_type:ty),* $(,)? }) => {
        #[cfg(not(hax))]
        #[derive(crate::acpi::zerocopy::FromBytes, crate::acpi::zerocopy::KnownLayout, crate::acpi::zerocopy::Immutable, crate::acpi::zerocopy::Unaligned)]
        $(#[$meta])*
        #[repr(C, packed)]
        #[mars_getters::unaligned_getters]
        $vis struct $name {
            $($field_vis $field_name : $field_type),*
        }

        #[cfg(hax)]
        $(#[$meta])*
        #[derive(crate::acpi::zerocopy::FromBytes, crate::acpi::zerocopy::KnownLayout, crate::acpi::zerocopy::Immutable, crate::acpi::zerocopy::Unaligned)]
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

pub struct SystemDescription {
    pub fadt: Option<&'static Fadt>,
    pub madt: Option<&'static Madt>,
    pub gtdt: Option<&'static Gtdt>,
    pub spcr: Option<&'static Spcr>,
    pub mcfg: Option<&'static Mcfg>,
    pub dbg2: Option<&'static SdtHeader>,
    pub dsdt_addr: u64,
}

impl SystemDescription {
    pub fn parse(xsdt: &SdtHeader) -> Self {
        let mut desc = SystemDescription {
            fadt: None,
            madt: None,
            gtdt: None,
            spcr: None,
            mcfg: None,
            dbg2: None,
            dsdt_addr: 0,
        };

        for header in XsdtIter::new(xsdt) {
            let sig = header.signature();
            match sig {
                "FACP" => {
                    let fadt = fadt_transmute(header);
                    desc.fadt = Some(fadt);
                    desc.dsdt_addr = fadt.x_dsdt();
                }
                "APIC" => desc.madt = Some(madt_transmute(header)),
                "GTDT" => desc.gtdt = Some(gtdt_transmute(header)),
                "SPCR" => desc.spcr = Some(spcr_transmute(header)),
                "MCFG" => desc.mcfg = Some(mcfg_transmute(header)),
                "DBG2" => desc.dbg2 = Some(header),
                _ => {}
            }
        }
        desc
    }

    /// search for hardware
    pub fn populate(tree: &mut DeviceTree) {}
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

#[attributes]
#[opaque]
#[ensures(|result| result.header == *input)]
fn fadt_transmute(input: &'static SdtHeader) -> &'static Fadt {
    unsafe { core::mem::transmute(input) }
}

#[attributes]
#[opaque]
#[ensures(|result| result.header == *input)]
fn madt_transmute(input: &'static SdtHeader) -> &'static Madt {
    unsafe { core::mem::transmute(input) }
}

#[attributes]
#[opaque]
#[ensures(|result| result.header == *input)]
fn gtdt_transmute(input: &'static SdtHeader) -> &'static Gtdt {
    unsafe { core::mem::transmute(input) }
}

#[attributes]
#[opaque]
#[ensures(|result| result.header == *input)]
fn spcr_transmute(input: &'static SdtHeader) -> &'static Spcr {
    unsafe { core::mem::transmute(input) }
}

#[attributes]
#[opaque]
#[ensures(|result| result.header == *input)]
fn mcfg_transmute(input: &'static SdtHeader) -> &'static Mcfg {
    unsafe { core::mem::transmute(input) }
}
