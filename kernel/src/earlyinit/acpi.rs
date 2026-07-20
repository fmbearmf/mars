use core::{ops::Range, ptr::NonNull, sync::atomic::Ordering};

use aarch64_cpu::registers::{MPIDR_EL1, Readable};
use alloc::{string::String, vec, vec::Vec};
use atomic_refcell::AtomicRefMut;
use klib::{
    cpu_interface::CpuTopologyId,
    hardware::{
        device::{DeviceClass, DeviceInitPriority, DeviceTree},
        resource::Resource,
    },
    interrupt::{GicdRegisters, GicrRegisters, gicv3::registers::gic::GicrTyper},
    per_cpu::PerCpu,
    pm::page::mapper::AddressTranslator,
    smccc::USE_HVC,
};
use mars_acpi_driver::acpi::{
    fadt::Fadt,
    gtdt::Gtdt,
    header::SdtHeader,
    madt::{GicCpuInterface, GicDistributor, GicRedistributor, Madt, MadtIter},
    xsdp::{Xsdp, XsdtIter},
};
use mars_models::memory::registers::volatile::PureReadable;
use uefi::table::cfg::ConfigTableEntry;
use uefi_raw::table::{configuration::ConfigurationTable, system::SystemTable};
use zerocopy::FromBytes;

use crate::{DEVICE_TREE, allocator::KernelAddressTranslator, earlyinit::platform::BootInfoToken};

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
pub fn acpi_init(token: &BootInfoToken) {
    use log::*;

    let bi = token.get();

    let st = bi.system_table_raw;

    info!("UEFI: System Table at {:p}", st);

    let cfg_table = config_table(st);

    let mut iter = cfg_table
        .iter()
        .filter(|t| t.guid == ConfigTableEntry::ACPI2_GUID);

    let xsdp = iter.next().expect("no ACPI2 table").address as *const Xsdp;

    assert_eq!(iter.next(), None, "more than one ACPI2 table?");

    let xsdp = Xsdp::try_from_addr(xsdp as _).unwrap_or_else(|e| panic!("XSDP err: {}", e));

    let xsdt: &SdtHeader = xsdp.xsdt().unwrap_or_else(|e| panic!("XSDT err: {}", e));

    let xsdt: &SdtHeader = unsafe {
        &*(KernelAddressTranslator.phys_to_dmap(xsdt as *const _ as _) as *const SdtHeader)
    };

    trace!("sdt: {:?}", xsdt);

    let xsdt_iter = XsdtIter::new(xsdt);
    for phys_table_bytes in xsdt_iter {
        let table_bytes: &[u8] = {
            let size = phys_table_bytes.len();
            let addr = KernelAddressTranslator
                .phys_to_dmap(phys_table_bytes as *const [u8] as *const () as _);

            unsafe { core::slice::from_raw_parts(addr, size) }
        };

        let (header, _): (&SdtHeader, _) =
            SdtHeader::ref_from_prefix(table_bytes).expect("table impossibly small");

        match &header.sig() {
            b"GTDT" => {
                trace!("    gtdt found");

                handle_gtdt(table_bytes);
            }
            b"APIC" => {
                trace!("    madt found");

                handle_madt(table_bytes);
            }
            b"FACP" => {
                trace!("    fadt found");

                handle_fadt(table_bytes);
            }
            _ => trace!("unrecognized ACPI table: {}", header.signature()),
        }
    }
}

fn handle_gtdt(table: &[u8]) {
    use log::*;

    let (gtdt, _) = Gtdt::ref_from_prefix(table).expect("invalid madt size");

    trace!("{:?}", gtdt);

    let platform_timer_count = gtdt.platform_timer_count();

    if platform_timer_count > 0 {
        use log::warn;
        warn!(
            "found {} platform timers. platform timer support is unimplemented.",
            platform_timer_count
        );
    }

    let mut dt = DEVICE_TREE.borrow_mut();
    dt.add_device(
        None,
        DeviceClass::Timer,
        vec![String::from("arm,armv8-timer")],
        vec![Resource::Irq(gtdt.virt_el1_gsiv())],
        Default::default(),
    );
}

fn handle_madt(table: &[u8]) {
    let (madt, _): (&Madt, &[u8]) = Madt::ref_from_prefix(table).expect("invalid madt size");

    let madt_iter = move || madt.entries();

    let mut dt = DEVICE_TREE.borrow_mut();

    if madt_iter().any(|(ty, _)| matches!(ty, 0xB | 0xC | 0xE)) {
        handle_gicv3(madt_iter, &mut dt);
    }
}

fn handle_fadt(table: &[u8]) {
    use log::*;

    let (fadt, _) = Fadt::ref_from_prefix(table).expect("invalid fadt size");

    let arm_flags = fadt.arm_boot_arch();
    let hvc = arm_flags.psci_use_hvc();

    trace!("    use HVC for PSCI?: {}", hvc);

    USE_HVC.store(hvc, Ordering::Relaxed);
}

fn handle_gicv3(madt: impl Fn() -> MadtIter, dt: &mut AtomicRefMut<'_, DeviceTree>) {
    use log::*;

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

                let cpu_id = CpuTopologyId::from_mpidr(gicc.mpidr());

                dt.add_device(
                    None,
                    DeviceClass::Cpu {
                        id: cpu_id,
                        acpi_uid: gicc.acpi_cpu_uid(),
                    },
                    Vec::new(),
                    Vec::new(),
                    DeviceInitPriority::Fundamental,
                );
            }
            0xC => {} // GICD
            0xE => {
                // GICR
                let gicr_handle: &GicRedistributor = GicRedistributor::ref_from_bytes(slice)
                    .expect("MADT GIC Redistributor entry contained wrong bytes");

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
        DeviceInitPriority::Fundamental,
    );
}
