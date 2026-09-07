use core::ptr::NonNull;

use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
use klib::{
    allocator_support::KernelAddressTranslator,
    hardware::{
        device::{DeviceNode, IrqFn},
        resource::Resource,
    },
    pm::page::mapper::AddressTranslator,
    sync::FairSpinlock,
    vm::{MAIR_DEVICE_INDEX, user::address_space::KERNEL_ADDRESS_SPACE},
};
use mars_pcie_driver::{address::Bdf, ecam::Ecam};

use crate::{controller::HostController, registers::XhciRegisters};

static CONTROLLER: FairSpinlock<Option<HostController>> = FairSpinlock::new(None);

pub fn with_controller<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut HostController) -> R,
{
    CONTROLLER.lock().as_mut().map(f)
}

pub fn poll() {
    with_controller(|ctrl| {
        ctrl.process_events();
    });
}

pub fn handle(device: &DeviceNode, _enable_irq: IrqFn, _disable_irq: IrqFn) {
    if let Err(e) = try_handle(device) {
        use log::*;
        error!("mars-xhci-driver error: {:?}", e);
    }
}

fn try_handle(device: &DeviceNode) -> Result<(), &'static str> {
    use log::*;

    let mmio_resource = device
        .resources
        .iter()
        .find_map(|res| match res {
            Resource::Mmio { range } => Some(range.clone()),
            _ => None,
        })
        .ok_or("xhci requires an MMIO resource")?;

    let (ecam, bdf) = device
        .resources
        .iter()
        .find_map(|res| match res {
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
        })
        .ok_or("xhci: missing PciEcam resource")?;

    info!(
        "xhci: found controller {} at {:#x}",
        bdf, mmio_resource.start
    );

    ecam.enable_memory_space(bdf);
    ecam.enable_bus_master(bdf);

    let phys_start = mmio_resource.start;
    let phys_size = mmio_resource.end - mmio_resource.start;
    let virt_start = KernelAddressTranslator.phys_to_dmap(phys_start) as usize;

    {
        let mut cursor = KERNEL_ADDRESS_SPACE.lock(core::range::Range::from(
            virt_start..(virt_start + phys_size),
        ));
        cursor.map(
            phys_start as _,
            AccessPermission::PrivilegedReadWrite,
            Shareability::OuterShareable,
            true,
            true,
            MAIR_DEVICE_INDEX,
        );
    }

    let ptr = NonNull::new(virt_start as *mut u8).ok_or("null MMIO base")?;
    let regs = unsafe { XhciRegisters::new(ptr) };

    let host = HostController::init(regs)?;
    info!("xhci: initialized host controller");

    let mut lock = CONTROLLER.lock();
    if lock.is_none() {
        lock.replace(host);
    }

    Ok(())
}
