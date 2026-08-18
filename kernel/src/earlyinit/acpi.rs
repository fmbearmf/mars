use core::{ops::Range, ptr::NonNull, sync::atomic::Ordering};

use aarch64_cpu::registers::{MPIDR_EL1, Readable};
use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
use alloc::{format, string::String, vec, vec::Vec};
use atomic_refcell::AtomicRefMut;
use klib::{
    allocator_support::KernelAddressTranslator,
    cpu_interface::CpuTopologyId,
    hardware::{
        device::{DeviceClass, DeviceInitPriority, DeviceTree},
        resource::Resource,
    },
    interrupt::{GicdRegisters, GicrRegisters, GitsRegisters, gicv3::registers::gic::GicrTyper},
    per_cpu::PerCpu,
    pm::page::mapper::{AddressTranslator, map_page},
    smccc::USE_HVC,
    vm::{MAIR_DEVICE_INDEX, PAGE_SIZE},
};
use mars_acpi_aml_driver::{
    ast::{AmlTerm, AmlValue},
    parser::AmlParser,
};
use mars_acpi_driver::acpi::{
    fadt::Fadt,
    gtdt::Gtdt,
    header::SdtHeader,
    madt::{GicCpuInterface, GicDistributor, GicIts, GicRedistributor, Madt, MadtIter},
    mcfg::Mcfg,
    xsdp::{Xsdp, XsdtIter},
};
use mars_models::memory::registers::volatile::PureReadable;
use mars_pcie_driver::{ecam::Ecam, scan::enumerate_segment};
use uefi::table::cfg::ConfigTableEntry;
use uefi_raw::table::{configuration::ConfigurationTable, system::SystemTable};
use zerocopy::FromBytes;

use crate::{DEVICE_TREE, KERNEL_ADDRESS_SPACE, earlyinit::platform::BootInfoToken};

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
            b"MCFG" => {
                trace!("    mcfg found");
                handle_mcfg(table_bytes);
            }
            _ => trace!("unrecognized ACPI table: {}", header.signature()),
        }
    }
}

fn handle_mcfg(table: &'static [u8]) {
    use log::*;

    let (mcfg, _) = Mcfg::ref_from_prefix(table).expect("invalid mcfg size");
    let mut dt = DEVICE_TREE.borrow_mut();

    for alloc in mcfg.allocations() {
        let bus_count = (alloc.end_bus_num() as usize - alloc.start_bus_num() as usize) + 1;
        let ecam_size = bus_count * (1024 * 1024); // 32 dev * 8 func * 4KiB
        let phys_base = alloc.base_addr();
        let va_start = KernelAddressTranslator.phys_to_dmap(phys_base as usize) as usize;
        let va_end = va_start + ecam_size;

        {
            trace!(
                "ACPI: Mapping PCIe ECAM Segment {} [Phys: {:#018x}..{:#018x}] -> [Vir: {:#018x}..{:#018x}] [{} MiB]",
                alloc.pci_segment_group(),
                phys_base,
                phys_base + (ecam_size as u64),
                va_start,
                va_end,
                ecam_size / (1024 * 1024)
            );

            // global AddressSpace unusable:
            // this memory region most likely won't have been included in the firmware memory map.
            // therefore they must be added via `map_page`
            // maybe add PCIe regions to page descriptors in the future, if userspace needs them. currently unnecessary.
            let root = unsafe { KERNEL_ADDRESS_SPACE.root_mut() };

            for offset in (0..ecam_size).step_by(PAGE_SIZE) {
                map_page(
                    root,
                    phys_base as usize + offset,
                    va_start + offset,
                    AccessPermission::PrivilegedReadWrite,
                    Shareability::OuterShareable,
                    true,
                    true,
                    MAIR_DEVICE_INDEX,
                    &KERNEL_ADDRESS_SPACE.allocator,
                    &KernelAddressTranslator,
                );
            }
        }

        debug!(
            "ACPI: Found PCIe Segment {} Base {:#018X} Bus {}..={}",
            alloc.pci_segment_group(),
            alloc.base_addr(),
            alloc.start_bus_num(),
            alloc.end_bus_num()
        );

        let ecam = Ecam::new(
            phys_base,
            alloc.pci_segment_group(),
            alloc.start_bus_num(),
            alloc.end_bus_num(),
        );

        enumerate_segment(&ecam, &mut dt);
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

    let (fadt, _) = Fadt::ref_from_prefix(table)
        .map_err(|_| "invalid fadt size")
        .unwrap();

    let arm_flags = fadt.arm_boot_arch();
    let hvc = arm_flags.psci_use_hvc();

    trace!("    use HVC for PSCI?: {}", hvc);

    USE_HVC.store(hvc, Ordering::Relaxed);

    let dsdt_phys_addr = if fadt.x_dsdt() != 0 {
        fadt.x_dsdt() as usize
    } else {
        fadt.dsdt() as usize
    };

    let dsdt_addr = KernelAddressTranslator.phys_to_dmap(dsdt_phys_addr) as *const u8;

    let dsdt_bytes = unsafe {
        let header_ptr = dsdt_addr as *const SdtHeader;
        let len = (*header_ptr).len() as usize;

        core::slice::from_raw_parts(dsdt_addr, len)
    };

    handle_dsdt(dsdt_bytes);
}

fn handle_dsdt(table: &[u8]) {
    use log::*;

    let (header, aml_bytes) = match SdtHeader::ref_from_prefix(table) {
        Ok(v) => v,
        Err(e) => {
            error!("DSDT header too small: {}", e);
            return;
        }
    };

    if &header.sig() != b"DSDT" {
        error!(
            "ACPI: Invalid DSDT table signature: \"{}\"",
            core::str::from_utf8(&header.sig()).unwrap()
        );
        return;
    }

    let mut root_parser = AmlParser::new(aml_bytes);

    debug!("ACPI: DSDT AML length = {}", aml_bytes.len());

    let mut output = String::new();
    format_aml_stream(&mut root_parser, 0, &mut output).unwrap();
    debug!("{}", output);
}

fn aml_stream(parser: &mut AmlParser, depth: usize) -> Result<(), &'static str> {
    while let Some(term) = parser.parse_next()? {
        match term {
            AmlTerm::Scope { name, mut contents } => {
                aml_stream(&mut contents, depth + 1)?;
            }
            AmlTerm::Device { name, mut contents } => {
                aml_stream(&mut contents, depth + 1)?;
            }
            AmlTerm::Name { name, value } => {}
            AmlTerm::Method { name, flags, code } => {}
            AmlTerm::OpRegion { name } => {}
            AmlTerm::Field => {}
            AmlTerm::UnsupportedOpcode(_op) => {}
        }
    }
    Ok(())
}

fn format_aml_value(val: &AmlValue, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    match val {
        AmlValue::Zero => out.push_str("Zero"),
        AmlValue::One => out.push_str("One"),
        AmlValue::Ones => out.push_str("Ones"),
        AmlValue::Integer(val) => out.push_str(&format!("{val:#X}")),
        AmlValue::String(s) => out.push_str(&format!("\"{s}\"")),
        AmlValue::NamePath(path) => out.push_str(&format!("{path}")),
        AmlValue::Buffer(buf) => out.push_str(&format!("Buffer ({}) {{ ... }}", buf.len())),
        AmlValue::Package(elems) => {
            if elems.is_empty() {
                out.push_str("Package (0x00) {}");
            } else {
                out.push_str(&format!("Package ({:#04X}) {{\n", elems.len()));
                let inner_indent = "  ".repeat(depth + 1);
                for (i, elem) in elems.iter().enumerate() {
                    out.push_str(&inner_indent);
                    format_aml_value(elem, depth + 1, out);
                    if i + 1 < elems.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(&format!("{indent}}}"));
            }
        }
    }
}

fn format_aml_stream(
    parser: &mut AmlParser,
    depth: usize,
    out: &mut String,
) -> Result<(), &'static str> {
    let indent = "  ".repeat(depth);

    while let Some(term) = parser.parse_next()? {
        match term {
            AmlTerm::Scope { name, mut contents } => {
                out.push_str(&format!("{indent}Scope ({name}) {{\n"));
                format_aml_stream(&mut contents, depth + 1, out)?;
                out.push_str(&format!("{indent}}}\n"));
            }
            AmlTerm::Device { name, mut contents } => {
                out.push_str(&format!("{indent}Device ({name}) {{\n"));
                format_aml_stream(&mut contents, depth + 1, out)?;
                out.push_str(&format!("{indent}}}\n"));
            }
            AmlTerm::Name { name, value } => {
                out.push_str(&format!("{indent}Name ({name}, "));
                format_aml_value(&value, depth, out);
                out.push_str(")\n");
            }
            AmlTerm::Method { name, flags, code } => {
                let arg_count = flags & 0x07;
                let serialized = if (flags & 0x08) != 0 {
                    "Serialized"
                } else {
                    "NotSerialized"
                };

                out.push_str(&format!(
                    "{indent}Method ({name}, {arg_count}, {serialized}) [Bytecode: {} bytes]\n",
                    code.len()
                ));
            }
            AmlTerm::OpRegion { name } => {
                out.push_str(&format!("{indent}OperationRegion ({name})\n"));
            }
            AmlTerm::Field => {
                out.push_str(&format!("{indent}Field (...)\n"));
            }
            AmlTerm::UnsupportedOpcode(op) => {
                out.push_str(&format!("{indent}// Unknown Opcode: {op:#04X}\n"));
            }
        }
    }

    Ok(())
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
        .map_err(|_| "MADT GIC Distributor entry contained wrong bytes")
        .unwrap();

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

    for (_, slice) in madt().filter(|(entry_type, _)| matches!(entry_type, 0xB)) {
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

    for (_, slice) in madt().filter(|(entry_type, _)| matches!(entry_type, 0xE)) {
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
                    ..(gicr_regs as *const GicrRegisters as usize + size_of::<GicrRegisters>()),
            });

            redistributor_count += 1;

            if last {
                break;
            }
        }
    }

    for (_, slice) in madt().filter(|(entry_type, _)| matches!(entry_type, 0xF)) {
        // ITS
        let gic_its =
            GicIts::ref_from_bytes(slice).expect("MADT GIC ITS entry contained wrong bytes");

        let base = gic_its.phys_base();
        if base != 0 {
            gic_resources.push(Resource::Mmio {
                range: (base as usize)..(base as usize + size_of::<GitsRegisters>()),
            })
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
