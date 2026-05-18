use core::ptr::NonNull;

use alloc::boxed::Box;
use klib::pm::page::mapper::AddressTranslator;
use log::{debug, trace};
use mars_acpi_driver::acpi::{
    header::SdtHeader,
    madt::{GicDistributor, Madt},
    xsdp::{Xsdp, XsdtIter},
};
use protocol::BootInfo;
use uefi::table::cfg::ConfigTableEntry;
use uefi_raw::table::{configuration::ConfigurationTable, system::SystemTable};
use zerocopy::{FromBytes, IntoBytes};

use crate::{BOOT_INFO, allocator::KernelAddressTranslator, busy_loop_ret};

fn config_table(st: NonNull<SystemTable>) -> &'static [ConfigTableEntry] {
    let st = KernelAddressTranslator.phys_to_dmap(st.as_ptr() as _) as *const SystemTable;
    let st = unsafe { &*st };

    let ct = st.configuration_table;
    if ct.is_null() {
        return &[];
    }

    let ct = KernelAddressTranslator.phys_to_dmap(ct as _) as *const ConfigurationTable;
    let ct = ct as *const ConfigTableEntry;

    let len = st.number_of_configuration_table_entries;

    unsafe { core::slice::from_raw_parts(ct, len) }
}

#[allow(static_mut_refs, reason = "singlethreaded")]
pub fn acpi_init() {
    let bi = unsafe { BOOT_INFO.assume_init_ref() };

    let st = bi.system_table_raw;

    debug!("st: {:p}", st);

    let cfg_table = config_table(st);

    let mut iter = cfg_table
        .iter()
        .filter(|t| t.guid == ConfigTableEntry::ACPI2_GUID);

    let xsdp = iter.next().expect("no ACPI2 table").address as *const Xsdp;

    assert_eq!(iter.next(), None, "more than one ACPI2 table?");

    let xsdp = Xsdp::try_from_addr(xsdp as _).unwrap_or_else(|e| panic!("XSDP err: {}", e));

    trace!("xsdp found at {:#p}", xsdp);

    let xsdt: &SdtHeader = xsdp.xsdt().unwrap_or_else(|e| panic!("XSDT err: {}", e));

    trace!("xsdt found at {:#p}", xsdt);

    let xsdt: &SdtHeader = unsafe {
        &*(KernelAddressTranslator.phys_to_dmap(xsdt as *const _ as _) as *const SdtHeader)
    };

    trace!("xsdt offset to virtual {:#p}", xsdt);

    debug!("xsdt: {:#?}", xsdt);

    let xsdt_iter = XsdtIter::new(xsdt);
    for phys_table_bytes in xsdt_iter {
        let table_bytes: &[u8] = {
            let size = phys_table_bytes.len();
            let addr = KernelAddressTranslator
                .phys_to_dmap(phys_table_bytes as *const [u8] as *const () as _);

            unsafe { core::slice::from_raw_parts(addr, size) }
        };

        trace!(
            "xsdt entry @ {:#p}",
            table_bytes as *const [u8] as *const ()
        );
        let (header, _): (&SdtHeader, _) =
            SdtHeader::ref_from_prefix(table_bytes).expect("table impossibly small");

        match &header.sig() {
            b"APIC" => {
                trace!("    madt found");

                handle_madt(table_bytes);
            }
            _ => trace!("unrecognized root ACPI table: {}", header.signature()),
        }
    }
}

fn handle_madt(table: &[u8]) {
    let (madt, _entries): (&Madt, &[u8]) = Madt::ref_from_prefix(table).expect("invalid madt size");
    for subtable in madt.entries() {
        match subtable.0 {
            12 => {
                // GIC distributor
                let gicd = GicDistributor::ref_from_bytes(subtable.1)
                    .expect("MADT GIC Distributor entry contained wrong bytes for a distributor");
                trace!("GIC distributor: {:#x?}", gicd);
            }
            _ => trace!("unrecognized madt subtable type: {}", subtable.0),
        }
    }
}
