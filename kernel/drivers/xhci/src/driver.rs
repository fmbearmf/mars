use alloc::{boxed::Box, format, vec::Vec};
use core::ptr::NonNull;

use klib::{
    allocator_support::KernelAddressTranslator,
    block::registry::register_hardware_device,
    hardware::{
        device::{DeviceNode, IrqFn},
        resource::{Resource, pci_bar_range},
    },
    interrupt::{
        InterruptError,
        gicv3::{IrqHandler, IrqTarget},
        singleton::get_interrupt_controller,
    },
    pm::page::mapper::AddressTranslator,
    strange::KernelPtr48,
    sync::FairSpinlock,
    vm::map_mmio,
};
use mars_pcie_driver::{
    address::Bdf,
    ecam::Ecam,
    interrupt::{MsixTable, enable_msix, get_msix_info},
};

use crate::{controller::HostController, mass_storage::MassStorage, registers::XhciRegisters};

struct RegisteredController {
    phys_base: usize,
    host: HostController,
}

static CONTROLLERS: FairSpinlock<Vec<RegisteredController>> = FairSpinlock::new(Vec::new());

pub fn with_controller<F, R>(index: usize, f: F) -> Option<R>
where
    F: FnOnce(&mut HostController) -> R,
{
    CONTROLLERS
        .lock()
        .get_mut(index)
        .map(|controller| f(&mut controller.host))
}

fn xhci_interrupt_handler(_int_id: u32) -> Result<(), InterruptError> {
    poll();
    Ok(())
}

pub fn poll() {
    for controller in CONTROLLERS.lock().iter_mut() {
        controller.host.process_events();
    }
}

pub fn handle(device: &DeviceNode, enable_irq: IrqFn, _disable_irq: IrqFn) {
    if let Err(error) = try_handle(device, enable_irq) {
        log::error!("mars-xhci-driver error: {error}");
    }
}

fn try_handle(device: &DeviceNode, enable_irq: IrqFn) -> Result<(), &'static str> {
    let mmio_resource = device
        .resources
        .iter()
        .find_map(|resource| match resource {
            Resource::Mmio { range } if range.end > range.start => Some(range.clone()),
            _ => None,
        })
        .ok_or("xHCI requires an MMIO resource")?;
    let phys_base = mmio_resource.start;

    if CONTROLLERS
        .lock()
        .iter()
        .any(|controller| controller.phys_base == phys_base)
    {
        log::debug!("xhci: controller at {phys_base:#x} was already initialized");
        return Ok(());
    }

    let pci = device.resources.iter().find_map(|resource| match resource {
        Resource::PciEcam {
            segment,
            bus,
            device,
            function,
            ecam_phys_base,
            ecam_start_bus,
            ecam_end_bus,
        } => Some((
            Ecam::new(*ecam_phys_base, *segment, *ecam_start_bus, *ecam_end_bus),
            Bdf::new(*segment, *bus, *device, *function),
        )),
        _ => None,
    });

    if let Some((ecam, bdf)) = &pci {
        log::info!("xhci: found PCI controller {bdf} at {phys_base:#x}");
        ecam.enable_memory_space(*bdf);
        ecam.enable_bus_master(*bdf);
    } else {
        log::info!("xhci: found platform controller at {phys_base:#x}");
    }

    let size = mmio_resource.end - mmio_resource.start;
    let virt_base =
        unsafe { map_mmio(phys_base, size) }.map_err(|_| "failed to map xHCI MMIO")? as usize;

    let registers = unsafe {
        XhciRegisters::new(
            NonNull::new(virt_base as *mut u8).ok_or("null xHCI MMIO base")?,
            size,
        )?
    };
    let controller_index = CONTROLLERS.lock().len();
    let controller_irqs = register_controller_interrupts(device, pci.as_ref(), &mmio_resource)?;

    let mut host = HostController::init(registers, phys_base)?;
    let mut storage_devices = Vec::new();
    for port_id in 1..=host.max_ports {
        match host.enumerate_port(port_id) {
            Ok(Some(usb_device)) => {
                match MassStorage::probe(&mut host, controller_index, usb_device) {
                    Ok(Some(storage)) => storage_devices.push(storage),
                    Ok(None) => {}
                    Err(error) => {
                        log::warn!("xhci: USB storage probe failed on port {port_id}: {error}")
                    }
                }
            }
            Ok(None) => {}
            Err(error) => log::warn!("xhci: USB enumeration failed on port {port_id}: {error}"),
        }
    }

    {
        let mut controllers = CONTROLLERS.lock();
        if controllers
            .iter()
            .any(|controller| controller.phys_base == phys_base)
        {
            return Ok(());
        }
        controllers.push(RegisteredController { phys_base, host });
    }

    for irq in controller_irqs {
        if enable_irq(&Resource::Irq(irq)).is_err() {
            log::warn!("xhci: failed to enable interrupt {irq}; using polling");
        }
    }

    for (device_index, storage) in storage_devices.into_iter().enumerate() {
        let name = format!("usb-storage{controller_index}-{device_index}");
        register_hardware_device(name, Box::new(storage));
    }

    Ok(())
}

fn register_controller_interrupts(
    device: &DeviceNode,
    pci: Option<&(Ecam, Bdf)>,
    mapped_bar: &core::ops::Range<usize>,
) -> Result<Vec<u32>, &'static str> {
    if let Some((ecam, bdf)) = pci {
        if let Some(msix_info) = get_msix_info(ecam, *bdf) {
            let table_resource = pci_bar_range(&device.resources, msix_info.table_bir)
                .ok_or("xHCI MSI-X table BAR is missing")?;
            let table_size = msix_info.table_size as usize * 16;
            let table_end = msix_info
                .table_offset
                .checked_add(table_size as u32)
                .ok_or("xHCI MSI-X table range overflow")? as usize;
            let bar_size = table_resource
                .end
                .checked_sub(table_resource.start)
                .filter(|size| *size != 0)
                .ok_or("xHCI MSI-X table BAR is empty")?;
            if table_end > bar_size {
                return Err("xHCI MSI-X table exceeds its BAR");
            }
            let table_virt = (KernelAddressTranslator.phys_to_dmap(table_resource.start) as usize)
                .checked_add(msix_info.table_offset as usize)
                .ok_or("xHCI MSI-X table address overflow")?;
            if table_resource.start != mapped_bar.start || table_resource.end != mapped_bar.end {
                unsafe { map_mmio(table_resource.start, bar_size) }
                    .map_err(|_| "failed to map xHCI MSI-X table")?;
            }

            let mut table = unsafe {
                MsixTable::from_raw_parts(
                    NonNull::new(table_virt as *mut u8).ok_or("null xHCI MSI-X table")?,
                    msix_info.table_size as usize,
                )
            };
            let irqs = enable_msix(ecam, *bdf, &mut table)?;
            for &irq in &irqs {
                register_irq(irq)?;
            }
            if !irqs.is_empty() {
                return Ok(irqs);
            }
        }
    }

    let irq = device
        .resources
        .iter()
        .find_map(|resource| match resource {
            Resource::Irq(irq) => Some(*irq),
            _ => None,
        })
        .ok_or("xHCI controller has no usable interrupt")?;
    register_irq(irq)?;
    Ok(alloc::vec![irq])
}

fn register_irq(irq: u32) -> Result<(), &'static str> {
    let handler = IrqHandler {
        target: IrqTarget::Distributor,
        dispatch_fn: KernelPtr48::new(xhci_interrupt_handler as _)
            .map_err(|_| "xHCI interrupt handler pointer is not compressible")?,
    };
    get_interrupt_controller()
        .register_handler(irq, handler)
        .map_err(|_| "failed to register xHCI interrupt handler")
}
