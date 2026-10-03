use core::{ops::Range, ptr::NonNull, sync::atomic::Ordering};

use alloc::{boxed::Box, string::String, vec, vec::Vec};
use atomic_refcell::AtomicRefMut;
use klib::{
    allocator_support::KernelAddressTranslator,
    cpu_interface::CpuTopologyId,
    hardware::{
        device::{DeviceClass, DeviceInitPriority, DeviceTree},
        resource::{DmaRemapping, Resource},
    },
    interrupt::{GicdRegisters, GicrRegisters, GitsRegisters},
    per_cpu::PerCpu,
    pm::page::mapper::AddressTranslator,
    smccc::USE_HVC,
    vm::map_mmio,
};
use mars_acpi_aml_driver::{device::TreeBuilder, parser::AmlParser};
use mars_acpi_driver::acpi::{
    fadt::Fadt,
    gtdt::Gtdt,
    header::SdtHeader,
    iort::{Iort, Translation},
    madt::{GicCpuInterface, GicDistributor, GicIts, GicRedistributor, Madt, MadtIter},
    mcfg::Mcfg,
    xsdp::{Xsdp, XsdtIter},
};
use mars_pcie_driver::{address::Bdf, ecam::Ecam, scan::enumerate_segment};
use uefi::table::cfg::ConfigTableEntry;
use uefi_raw::table::{configuration::ConfigurationTable, system::SystemTable};
use zerocopy::FromBytes;

use crate::{DEVICE_TREE, earlyinit::platform::BootInfoToken};

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
        .map(|p| {
            (
                KernelAddressTranslator.phys_to_dmap(p.address as _) as *const Xsdp,
                p,
            )
        })
        .filter(|t| t.1.guid == ConfigTableEntry::ACPI2_GUID);

    let xsdp = iter.next().expect("no ACPI2 table").0;

    assert_eq!(iter.next(), None, "more than one ACPI2 table?");

    let xsdp = Xsdp::try_from_addr(xsdp as _).unwrap_or_else(|e| panic!("XSDP err: {}", e));

    let xsdt: &SdtHeader = xsdp
        .xsdt(|addr| match addr {
            0 => 0usize,
            any => KernelAddressTranslator.phys_to_dmap(any) as _,
        })
        .unwrap_or_else(|e| panic!("XSDT err: {}", e));

    let xsdt: &SdtHeader = unsafe {
        &*(KernelAddressTranslator.phys_to_dmap(xsdt as *const _ as _) as *const SdtHeader)
    };

    trace!("sdt: {:?}", xsdt);

    let xsdt_iter = XsdtIter::new(
        xsdt,
        Box::new(|addr| match addr {
            0 => 0usize,
            any => KernelAddressTranslator.phys_to_dmap(any) as _,
        }),
    );
    let mut dsdt_table = None;
    let mut ssdt_tables = Vec::new();
    let mut iort_table = None;
    let mut duplicate_iort = false;
    for phys_table_bytes in xsdt_iter {
        let table_bytes: &'static [u8] = {
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
                dsdt_table = handle_fadt(table_bytes);
            }
            b"MCFG" => {
                trace!("    mcfg found");
                handle_mcfg(table_bytes);
            }
            b"SSDT" => {
                trace!("    ssdt found");
                ssdt_tables.push(table_bytes);
            }
            b"IORT" => {
                if iort_table.replace(table_bytes).is_some() {
                    duplicate_iort = true;
                }
            }
            _ => trace!("unrecognized ACPI table: {}", header.signature()),
        }
    }

    let identity_dma = handle_aml_tables(dsdt_table.into_iter().chain(ssdt_tables));
    // MCFG and IORT ordering in the XSDT must not affect PCI DMA discovery.
    if duplicate_iort {
        error!("ACPI: multiple IORT tables; PCI DMA topology remains unresolved");
    } else if let Some(table) = iort_table {
        if identity_dma {
            handle_iort(table);
        } else {
            error!("ACPI: AML DMA addressing is unresolved; refusing identity-DMA devices");
        }
    }
}

fn handle_iort(table: &[u8]) {
    let iort = match Iort::parse(table) {
        Ok(iort) => iort,
        Err(error) => {
            log::error!("ACPI: invalid IORT: {error:?}; PCI DMA topology remains unresolved");
            return;
        }
    };
    let mut tree = DEVICE_TREE.borrow_mut();
    for node in &mut tree.nodes {
        let pci = node.resources.iter().find_map(|resource| match resource {
            Resource::PciEcam {
                segment,
                bus,
                device,
                function,
                ..
            } => Some(Bdf::new(*segment, *bus, *device, *function)),
            _ => None,
        });
        let Some(pci) = pci else { continue };
        let route = match iort.pci_route(pci.segment, pci.requester_id() as u16) {
            Ok(route) => route,
            Err(error) => {
                log::warn!("PCI {pci}: unresolved IORT DMA route: {error:?}");
                continue;
            }
        };
        let remapping = match route.translation {
            Translation::None => DmaRemapping::NoIommu,
            Translation::Smmu {
                base,
                span,
                model,
                flags,
                stream_id,
            } => DmaRemapping::ArmSmmu {
                base,
                span,
                model,
                flags,
                stream_id,
            },
            Translation::SmmuV3 {
                base,
                flags,
                model,
                stream_id,
            } => DmaRemapping::ArmSmmuV3 {
                base,
                flags,
                model,
                stream_id,
            },
        };
        log::debug!("PCI {pci}: IORT {route:?}");
        node.resources.push(Resource::PciDmaTopology {
            address_bits: route.address_bits,
            coherent: route.coherent,
            remapping,
        });
    }
}

fn handle_mcfg(table: &'static [u8]) {
    use log::*;

    let (mcfg, _) = Mcfg::ref_from_prefix(table).expect("invalid mcfg size");
    let mut dt = DEVICE_TREE.borrow_mut();

    for alloc in mcfg.allocations() {
        let start_bus = alloc.start_bus_num() as usize;
        let end_bus = alloc.end_bus_num() as usize;
        let Some(bus_count) = end_bus.checked_sub(start_bus).map(|count| count + 1) else {
            error!("ACPI: invalid ECAM bus range");
            continue;
        };
        let ecam_size = bus_count * (1024 * 1024); // 32 dev * 8 func * 4KiB

        let phys_base_bus0 = alloc.base_addr() as usize;
        let Some(phys_base) = phys_base_bus0.checked_add(start_bus << 20) else {
            error!("ACPI: ECAM address overflow");
            continue;
        };

        let va_start = match unsafe { map_mmio(phys_base, ecam_size) } {
            Ok(pointer) => pointer as usize,
            Err(error) => {
                error!("ACPI: failed to map ECAM: {error:?}");
                continue;
            }
        };
        let va_end = va_start + ecam_size;

        {
            trace!(
                "ACPI: Mapping PCIe ECAM Segment {} [Phys: {:#018x}..{:#018x}] -> [Vir: {:#018x}..{:#018x}] [{} MiB]",
                alloc.pci_segment_group(),
                phys_base,
                phys_base + ecam_size,
                va_start,
                va_end,
                ecam_size / (1024 * 1024)
            );
        }

        debug!(
            "ACPI: Found PCIe Segment {} Base {:#018X} Bus {}..={}",
            alloc.pci_segment_group(),
            alloc.base_addr(),
            alloc.start_bus_num(),
            alloc.end_bus_num()
        );

        let ecam = Ecam::new(
            phys_base as u64,
            alloc.pci_segment_group(),
            alloc.start_bus_num(),
            alloc.end_bus_num(),
        );

        enumerate_segment(
            &ecam,
            phys_base as u64,
            alloc.start_bus_num(),
            alloc.end_bus_num(),
            &mut dt,
        );
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

    let gsiv = hal::timer::interrupt_from_acpi(table).expect("invalid timer descriptor");

    let mut dt = DEVICE_TREE.borrow_mut();
    dt.add_device(
        None,
        DeviceClass::Timer,
        vec![String::from("arm,armv8-timer")],
        vec![Resource::Irq(gsiv)],
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

fn handle_fadt(table: &[u8]) -> Option<&'static [u8]> {
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

    let dsdt_bytes: &'static [u8] = unsafe {
        let header_ptr = dsdt_addr as *const SdtHeader;
        let len = (*header_ptr).len() as usize;
        core::slice::from_raw_parts(dsdt_addr, len)
    };

    Some(dsdt_bytes)
}

fn handle_aml_tables(tables: impl IntoIterator<Item = &'static [u8]>) -> bool {
    use log::*;

    let mut tree = DEVICE_TREE.borrow_mut();
    let old_device_count = tree.nodes.len();
    let mut builder = TreeBuilder::new(&mut tree);
    let mut complete = true;
    let mut saw_dsdt = false;

    for table in tables {
        let (header, aml_bytes) = match SdtHeader::ref_from_prefix(table) {
            Ok(value) => value,
            Err(error) => {
                error!("ACPI AML table header is invalid: {error}");
                complete = false;
                continue;
            }
        };
        let signature = header.sig();
        if signature != *b"DSDT" && signature != *b"SSDT" {
            error!("ACPI AML table has an unexpected signature");
            complete = false;
            continue;
        }
        saw_dsdt |= signature == *b"DSDT";

        let mut parser = AmlParser::new(aml_bytes);
        let mut terms = Vec::new();
        while !parser.is_empty() {
            match parser.parse_next() {
                Ok(Some(term)) => terms.push(term),
                Ok(None) => break,
                Err(error) => {
                    warn!("ACPI AML parsing stopped after a partial table: {error}");
                    complete = false;
                    break;
                }
            }
        }

        if let Err(error) = builder.process_terms(terms, "\\", None) {
            warn!("ACPI device-tree construction stopped early: {error}");
            complete = false;
        }
    }

    // _DMA must be evaluated before using CPU physical addresses as PCI addresses.
    // The AML interpreter does not yet implement translated DMA windows.
    let identity_dma = complete && saw_dsdt && !builder.has_dma_translation();
    drop(builder);
    info!(
        "ACPI AML: discovered {} device node(s)",
        tree.nodes.len() - old_device_count
    );
    identity_dma
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

    let current_topo = CpuTopologyId::current();
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

    let gic_version = gicd.gic_version();
    if gic_version < 3 {
        error!(
            "    GIC version is less than 3 (unsupported): {}",
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

    // GICv3 = 2 frames (128k), advance 1
    // GICv4 = 4 frames (256k), advance 2
    let step = if gic_version == 4 { 2 } else { 1 };

    for (_, slice) in madt().filter(|(entry_type, _)| matches!(entry_type, 0xE)) {
        // GICR
        let gicr_handle: &GicRedistributor = GicRedistributor::ref_from_bytes(slice)
            .expect("MADT GIC Redistributor entry contained wrong bytes");

        let gicr_block = gicr_handle
            .frames()
            .expect("MADT GIC Redistributor entry contained invalid GICR block");

        let mut i = 0;
        while i < gicr_block.len() {
            // break if every (known) CPU is accounted for
            if redistributor_count >= cpu_topologies.len() {
                break;
            }

            let gicr_frame = match gicr_block.get(i) {
                Some(f) => f,
                None => break,
            };

            let paddr = gicr_frame.reg as *const GicrRegisters as usize;

            gic_resources.push(Resource::Mmio {
                range: paddr..(paddr + size_of::<GicrRegisters>()),
            });

            redistributor_count += 1;
            i += step;
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
            redistributor_count: redistributor_count as _,
        },
        vec![String::from("arm,gic-v3")],
        gic_resources,
        DeviceInitPriority::Fundamental,
    );
}
