use core::ptr::NonNull;

use aarch64_cpu_ext::{
    asm::wfe,
    structures::tte::{AccessPermission, Shareability},
};
use klib::{
    allocator_support::KernelAddressTranslator,
    hardware::{
        device::{DeviceNode, IrqFn},
        resource::Resource,
    },
    interrupt::{
        InterruptError,
        gicv3::{IrqHandler, IrqTarget},
        singleton::get_interrupt_controller,
    },
    pm::page::mapper::AddressTranslator,
    strange::KernelPtr48,
    sync::FairSpinlock,
    vm::{MAIR_DEVICE_INDEX, user::address_space::KERNEL_ADDRESS_SPACE},
};
use mars_pcie_driver::{
    address::Bdf,
    ecam::Ecam,
    interrupt::{MsixTable, enable_msix, get_msix_info},
};

use crate::{controller::HostController, registers::XhciRegisters, trb::Trb};

static CONTROLLER: FairSpinlock<Option<HostController>> = FairSpinlock::new(None);

pub fn with_controller<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut HostController) -> R,
{
    CONTROLLER.lock().as_mut().map(f)
}

fn xhci_interrupt_handler(int_id: u32) -> Result<(), InterruptError> {
    use log::*;
    debug!("xhci: hi. received interrupt (ID {})", int_id);
    poll();
    Ok(())
}

pub fn poll() {
    with_controller(|ctrl| {
        ctrl.process_events();
    });
}

/// wait for completions
pub fn wait_for_event() -> Trb {
    loop {
        if let Some(Some(event)) = with_controller(|ctrl| ctrl.pop_event()) {
            return event;
        }
        wfe();
    }
}

pub fn handle(device: &DeviceNode, enable_irq: IrqFn, _disable_irq: IrqFn) {
    if let Err(e) = try_handle(device, enable_irq) {
        use log::*;
        error!("mars-xhci-driver error: {:?}", e);
    }
}

fn try_handle(device: &DeviceNode, enable_irq: IrqFn) -> Result<(), &'static str> {
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

    let msix_info = get_msix_info(&ecam, bdf).ok_or("xhci: device does not support MSI-X")?;

    let table_virt = if msix_info.table_bir == 0 {
        (virt_start + msix_info.table_offset as usize) as *mut u8
    } else {
        let bar_res = device
            .resources
            .iter()
            .filter_map(|res| match res {
                Resource::Mmio { range } => Some(range.clone()),
                _ => None,
            })
            .nth(msix_info.table_bir as usize)
            .ok_or("xhci: BAR for MSI-X table not found in device resources")?;

        let phys_start = bar_res.start;
        let phys_size = bar_res.end - bar_res.start;
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

        (virt_start + msix_info.table_offset as usize) as *mut u8
    };

    let mut msix_table = unsafe {
        MsixTable::from_raw_parts(
            NonNull::new(table_virt).ok_or("null MSI-X table base")?,
            msix_info.table_size as usize,
        )
    };

    let mapped_lpis = enable_msix(&ecam, bdf, &mut msix_table)?;

    if let Some(e0) = msix_table.entry(0) {
        info!(
            "xhci: MSI-X entry 0 -> Addr={:#x}, Data={:#x}, Masked={}",
            e0.address(),
            e0.data(),
            e0.is_masked()
        )
    }

    let ic = get_interrupt_controller();

    for &lpi in &mapped_lpis {
        let handler = IrqHandler {
            target: IrqTarget::Distributor,
            dispatch_fn: KernelPtr48::new(xhci_interrupt_handler as _)
                .map_err(|_| "xhci: interrupt handler pointer isn't compressable. fatal.")?,
        };

        ic.register_handler(lpi, handler)
            .map_err(|_| "xhci: failed to register LPI handler")?;

        enable_irq(&Resource::Irq(lpi)).map_err(|_| "xhci: failed to enable LPI in IC")?;
    }

    let host = HostController::init(regs)?;
    info!(
        "xhci: initialized host controller with {} MSI-X vector(s)",
        mapped_lpis.len()
    );

    {
        let mut lock = CONTROLLER.lock();
        if lock.is_none() {
            lock.replace(host);
        }
    }

    Ok(())
}
