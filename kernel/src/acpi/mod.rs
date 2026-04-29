use core::ptr::NonNull;

use klib::{acpi::xsdp::Xsdp, pm::page::mapper::AddressTranslator};
use log::{debug, trace};
use protocol::BootInfo;
use uefi::table::cfg::ConfigTableEntry;
use uefi_raw::table::{configuration::ConfigurationTable, system::SystemTable};

use crate::{BOOT_INFO, allocator::KernelAddressTranslator};

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

    let xsdp = unsafe { Xsdp::try_from_ptr(xsdp) }.unwrap_or_else(|e| panic!("XSDP err: {}", e));

    trace!("xsdp found at {:#p}", xsdp);

    let xsdt = xsdp.xsdt().unwrap_or_else(|e| panic!("XSDT err: {}", e));

    trace!("xsdt found at {:#p}", xsdt);
    debug!("xsdt: {:#?}", xsdt);
}

pub fn parse_gic_addresses(sys: &SystemDescription) -> Option<(u64, u64)> {
    let madt = sys.madt?;

    let mut gicd_phys_base = None;
    let mut gicr_phys_base = None;

    for (type_, slice) in madt.entries() {
        match type_ {
            MADT_GICD => {
                let (gicd, _) = GicDistributor::read_from_prefix(slice).unwrap();
                gicd_phys_base = Some(gicd.phys_base());
            }
            MADT_GICR => {
                let (gicr, _) = GicRedistributor::read_from_prefix(slice).unwrap();
                gicr_phys_base = Some(gicr.discovery_range_base());
            }
            _ => {}
        }
    }

    match (gicd_phys_base, gicr_phys_base) {
        (Some(d), Some(r)) => Some((d, r)),
        _ => None,
    }
}
