use core::{ops::Range, ptr::NonNull, sync::atomic::AtomicPtr};

use aarch64_cpu::registers::{MPIDR_EL1, Readable};
use alloc::{string::String, vec, vec::Vec};
use atomic_refcell::AtomicRefMut;
use klib::{
    cpu_interface::CpuTopologyId,
    hardware::{
        device::{DeviceClass, DeviceTree},
        resource::Resource,
    },
    interrupt::{GicdRegisters, GicrRegisters, gicv3::registers::gic::GicrTyper},
    per_cpu::PerCpu,
    pm::page::mapper::AddressTranslator,
};
use log::{debug, error, trace};
use mars_acpi_driver::acpi::{
    header::SdtHeader,
    madt::{GicCpuInterface, GicDistributor, GicRedistributor, Madt, MadtIter},
    xsdp::{Xsdp, XsdtIter},
};
use mars_models::memory::registers::volatile::PureReadable;
use uefi::table::cfg::ConfigTableEntry;
use uefi_raw::table::{configuration::ConfigurationTable, system::SystemTable};
use zerocopy::{FromBytes, IntoBytes};

use crate::{BOOT_INFO, DEVICE_TREE, allocator::KernelAddressTranslator};

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

    trace!("st: {:p}", st);

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

    trace!("xsdt: {:#?}", xsdt);

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

    let madt_iter = move || madt.entries();

    let mut dt = DEVICE_TREE.borrow_mut();

    if madt_iter().any(|(ty, _)| matches!(ty, 0xB | 0xC | 0xE)) {
        handle_gicv3(madt_iter, &mut dt);
    }
}

fn handle_gicv3(madt: impl Fn() -> MadtIter, dt: &mut AtomicRefMut<'_, DeviceTree>) {
    let mut cpu_topologies = Vec::new();
    for (_, slice) in madt().filter(|&(ty, _)| ty == 0xB) {
        let gicc: &GicCpuInterface = GicCpuInterface::ref_from_bytes(slice)
            .expect("MADT GIC CPU Interface entry contained wrong bytes");
        cpu_topologies.push(CpuTopologyId::from_mpidr(gicc.mpidr()));
    }

    PerCpu::init(cpu_topologies.len());

    let current_topo = CpuTopologyId::from_mpidr(MPIDR_EL1.get());
    for (i, &topo) in cpu_topologies.iter().enumerate() {
        if topo == current_topo {
            PerCpu::register_local(i).expect("invalid index");
            break;
        }
    }

    let mut gic_resources = Vec::new();
    let mut redistributor_count = 0;

    let gicd_entry_slice = madt()
        .find(|(entry_type, _)| *entry_type == 0xC)
        .map(|(_, slice)| slice)
        .expect("MADT didn't contain a GIC Distributor entry");

    let gicd: &GicDistributor = GicDistributor::ref_from_bytes(gicd_entry_slice)
        .expect("MADT GIC Distributor entry contained wrong bytes");

    trace!("    GIC distributor: {:#x?}", gicd);

    if gicd.gic_version() != 3 {
        error!(
            "    GIC version isn't 3 (unsupported): {}",
            gicd.gic_version()
        );
        unimplemented!();
    }

    let gicd_range: Range<usize> = {
        let base = gicd.phys_base();
        assert_ne!(base, 0, "GICD physical base is null");

        (base as usize)..(base as usize + size_of::<GicdRegisters>())
    };

    gic_resources.push(Resource::Mmio { range: gicd_range });

    for (entry_type, slice) in madt() {
        match entry_type {
            0xB => {
                // GICC
                let gicc: &GicCpuInterface = GicCpuInterface::ref_from_bytes(slice)
                    .expect("MADT GIC CPU Interface entry contained wrong bytes for a GICC");

                trace!("    GIC cpu interface: {:#x?}", gicc);

                let cpu_id = CpuTopologyId::from_mpidr(gicc.mpidr());

                dt.add_device(
                    None,
                    DeviceClass::Cpu {
                        id: cpu_id,
                        acpi_uid: gicc.acpi_cpu_uid(),
                    },
                    Vec::new(),
                    Vec::new(),
                );
            }
            0xC => {} // GICD
            0xE => {
                // GICR
                let gicr_handle: &GicRedistributor = GicRedistributor::ref_from_bytes(slice)
                    .expect("MADT GIC Redistributor entry contained wrong bytes");

                trace!("    gic redistributor block: {:#x?}", gicr_handle);

                let gicr_block = gicr_handle
                    .frames()
                    .expect("MADT GIC Redistributor entry contained invalid GICR block");

                for i in 0..gicr_block.len() {
                    let gicr_frame = match gicr_block.get(i) {
                        Some(f) => f,
                        None => break,
                    };

                    let gicr_regs = gicr_frame.reg;

                    let last = gicr_regs
                        .type_
                        .read_field_pure(GicrTyper::LastRedistributor);
                    let id = gicr_regs.type_.read_field_pure(GicrTyper::AffinityValue);

                    trace!("    gic redistributor #{}: {:#x?}", i, gicr_frame);

                    let redist_topo = CpuTopologyId::new(id);
                    let virt_gicr = KernelAddressTranslator
                        .phys_to_dmap(gicr_regs as *const GicrRegisters as usize)
                        as *mut GicrRegisters;

                    if let Some(i) = cpu_topologies.iter().position(|&t| t == redist_topo) {
                        //redistributors[i] = AtomicPtr::new(virt_gicr);
                        trace!("initialized redistributor of cpu #{}", i);
                    }

                    gic_resources.push(Resource::Mmio {
                        range: (gicr_regs as *const GicrRegisters as usize)
                            ..(gicr_regs as *const GicrRegisters as usize
                                + size_of::<GicrRegisters>()),
                    });

                    redistributor_count += 1;

                    if last {
                        break;
                    }
                }
            }
            _ => trace!("   unrecognized madt subtable type: {:x}", entry_type),
        }
    }

    dt.add_device(
        None,
        DeviceClass::GicV3 {
            redistributor_count,
        },
        vec![String::from("arm,gic-v3")],
        gic_resources,
    );
}
