pub mod fadt;
pub mod gtdt;
pub mod header;
pub mod madt;
pub mod mcfg;
pub mod rsdp;
pub mod spcr;

use core::{ffi::c_void, fmt, mem, ptr, slice, str::from_utf8};

use uefi::{system, table::cfg::ACPI2_GUID};

use getters::unaligned_getters;

use fadt::Fadt;
use gtdt::Gtdt;
use header::SdtHeader;
use madt::Madt;
use mcfg::Mcfg;
use rsdp::XsdtIter;
use spcr::Spcr;

#[repr(C, packed)]
#[unaligned_getters]
#[derive(Debug, Clone, Copy)]
pub struct GenericAddress {
    pub address_space_id: u8,
    pub register_bit_width: u8,
    pub register_bit_offset: u8,
    pub access_size: u8,
    pub address: u64,
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
                "FACP" => unsafe {
                    let fadt = &*(header as *const _ as *const Fadt);
                    desc.fadt = Some(fadt);
                    desc.dsdt_addr = fadt.x_dsdt();
                },
                "APIC" => unsafe { desc.madt = Some(&*(header as *const _ as *const Madt)) },
                "GTDT" => unsafe { desc.gtdt = Some(&*(header as *const _ as *const Gtdt)) },
                "SPCR" => unsafe { desc.spcr = Some(&*(header as *const _ as *const Spcr)) },
                "MCFG" => unsafe { desc.mcfg = Some(&*(header as *const _ as *const Mcfg)) },
                "DBG2" => unsafe { desc.dbg2 = Some(header) },
                _ => {}
            }
        }
        desc
    }
}

pub fn checksum(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
}
