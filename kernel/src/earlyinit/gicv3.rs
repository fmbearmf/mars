use core::{range::Range, sync::atomic::AtomicPtr};

use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
use alloc::{boxed::Box, vec::Vec};
use klib::{
    cpu_interface::Arm64InterruptInterface,
    hardware::{
        device::{DeviceClass, DeviceNode, IrqFn},
        resource::Resource,
    },
    interrupt::{GicdRegisters, GicrRegisters, gicv3::GicV3},
    pm::page::mapper::AddressTranslator,
    this_cpu,
    vm::MAIR_DEVICE_INDEX,
};
use zerocopy::FromBytes;

use crate::{
    KERNEL_ADDRESS_SPACE,
    allocator::KernelAddressTranslator,
    interrupt::{get_interrupt_controller, set_interrupt_controller},
};

pub fn secondary_handle(_node: &DeviceNode, _enable_irq: IrqFn, _disable_irq: IrqFn) {
    use log::*;

    let core_id = this_cpu!().id;

    info!("GICv3: Enabled on core {}.", core_id);
    get_interrupt_controller().init().unwrap();
}

pub fn handle(node: &DeviceNode, _enable_irq: IrqFn, _disable_irq: IrqFn) {
    use log::*;

    trace!("gicv3_handler: {:?}", node.compatible);

    let redistributor_count = match node.class {
        DeviceClass::GicV3 {
            redistributor_count,
        } => redistributor_count,
        _ => {
            error!("gicv3_handler: device isn't a `GicV3`?");
            return;
        }
    };

    let distributor = {
        let resource = node.resources.get(0);
        if resource.is_none() {
            error!("gicv3_handler: no distributor (0th element)");
            return;
        }

        let resource = resource.unwrap();

        let range = match resource {
            Resource::Mmio { range } => range,
            _ => {
                error!(
                    "gicv3_handler: unexpected resource on `GicV3` distributor: {:?}",
                    resource
                );
                return;
            }
        };

        let virt_start = KernelAddressTranslator.phys_to_dmap(range.start) as *mut u8;

        let size = range.end - range.start;

        let mut cursor = KERNEL_ADDRESS_SPACE.lock(Range::from(
            (virt_start as usize)..(virt_start as usize + size),
        ));
        cursor.map(
            range.start as _,
            AccessPermission::PrivilegedReadWrite,
            Shareability::OuterShareable,
            true,
            true,
            MAIR_DEVICE_INDEX,
        );

        let slice = unsafe { core::slice::from_raw_parts_mut(virt_start, size) };

        GicdRegisters::mut_from_bytes(slice).unwrap()
    };

    // 1..=redistributor_count + 1
    // ie skip the 1st element (distributor), and ignore anything after the last redistributor (ITS)
    let redistributors: Vec<AtomicPtr<GicrRegisters>> = node
        .resources
        .iter()
        .skip(1)
        .take(redistributor_count as usize)
        .filter_map(|redist| match redist {
            Resource::Mmio { range } => unsafe {
                let size = range.end - range.start;

                let virt_start = KernelAddressTranslator.phys_to_dmap(range.start) as *mut u8;
                let mut cursor = KERNEL_ADDRESS_SPACE.lock(Range::from(
                    (virt_start as usize)..(virt_start as usize + size),
                ));
                cursor.map(
                    range.start as _,
                    AccessPermission::PrivilegedReadWrite,
                    Shareability::OuterShareable,
                    true,
                    true,
                    MAIR_DEVICE_INDEX,
                );

                let slice = core::slice::from_raw_parts_mut(virt_start, size);

                let redist = GicrRegisters::mut_from_bytes(slice).unwrap();

                Some(AtomicPtr::new(redist as *mut _))
            },
            _ => {
                error!(
                    "gicv3_handler: unexpected resource on `GicV3` redistributor: {:?}",
                    redist
                );
                None
            }
        })
        .collect();

    let gicv3: Box<GicV3<'_, Arm64InterruptInterface>> = Box::new(GicV3::new(
        distributor,
        redistributors,
        Arm64InterruptInterface,
    ));

    trace!("set interrupt controller to GicV3");

    set_interrupt_controller(gicv3);

    // init the controller
    secondary_handle(node, _enable_irq, _disable_irq);
}
