use core::sync::atomic::AtomicPtr;

use alloc::{boxed::Box, vec::Vec};
use klib::{
    cpu_interface::Arm64InterruptInterface,
    hardware::{
        device::{DeviceClass, DeviceNode},
        resource::Resource,
    },
    interrupt::{GicdRegisters, GicrRegisters, gicv3::GicV3},
    pm::page::mapper::AddressTranslator,
};
use log::{debug, error, trace};
use mars_acpi_driver::acpi::madt::{GicRedistributor, GicrFrame};
use zerocopy::FromBytes;

use crate::{allocator::KernelAddressTranslator, interrupt::set_interrupt_controller};

pub fn gicv3_handler(node: &mut DeviceNode) {
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

        let slice = unsafe {
            let virt_start = KernelAddressTranslator.phys_to_dmap(range.start) as *mut u8;
            core::slice::from_raw_parts_mut(virt_start, range.end - range.start)
        };

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
                let virt_start = KernelAddressTranslator.phys_to_dmap(range.start) as *mut u8;

                let slice = core::slice::from_raw_parts_mut(virt_start, range.end - range.start);

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

    debug!("set interrupt controller to GicV3");

    set_interrupt_controller(gicv3);
}
