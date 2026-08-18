use alloc::{
    format,
    string::String,
    {vec, vec::Vec},
};
use klib::hardware::{
    device::{DeviceClass, DeviceInitPriority, DeviceTree},
    resource::Resource,
};
use log::*;

use crate::{
    address::Bdf,
    bar::{BarType, probe_bars},
    ecam::Ecam,
};

pub fn enumerate_segment(ecam: &Ecam, dt: &mut DeviceTree) {
    // standard init
    scan_bus(ecam, ecam.start_bus, dt);
}

fn scan_bus(ecam: &Ecam, bus: u8, dt: &mut DeviceTree) {
    for device in 0..32 {
        let bdf = Bdf::new(ecam.segment, bus, device, 0);
        let vendor = ecam.read_u16(bdf, 0x00);

        if vendor == !0 {
            continue; // no device
        }

        scan_function(ecam, bdf, dt);

        let header_type = ecam.read_u8(bdf, 0x0E);
        if (header_type & 0x80) != 0 {
            // multi function
            for function in 1..8 {
                let func_bdf = Bdf::new(ecam.segment, bus, device, function);
                let func_vendor = ecam.read_u16(func_bdf, 0x00);

                if func_vendor != !0 {
                    scan_function(ecam, func_bdf, dt);
                }
            }
        }
    }
}

fn scan_function(ecam: &Ecam, bdf: Bdf, dt: &mut DeviceTree) {
    let vendor_id = ecam.read_u16(bdf, 0x00);
    let device_id = ecam.read_u16(bdf, 0x02);
    let class_code = ecam.read_u8(bdf, 0x0B);
    let subclass = ecam.read_u8(bdf, 0x0A);
    let prog_if = ecam.read_u8(bdf, 0x09);
    let header_type = ecam.read_u8(bdf, 0x0E) & 0x07;

    info!(
        "Found PCI Device {} [{:04x}:{:04x}] Class {:02x}.{:02x} ProgIf {:02x} Header {:02x}",
        bdf, vendor_id, device_id, class_code, subclass, prog_if, header_type
    );

    if header_type == 0x00 {
        // endpoint device
        let bars = probe_bars(ecam, bdf);
        let mut resources = Vec::new();

        for bar in &bars {
            match bar {
                BarType::Memory32 { address, size, .. } => {
                    let address = *address as usize;
                    let size = *size as usize;
                    resources.push(Resource::Mmio {
                        range: address..(address + size),
                    });
                }
                BarType::Memory64 { address, size, .. } => {
                    let address = *address as usize;
                    let size = *size as usize;
                    resources.push(Resource::Mmio {
                        range: address..(address + size),
                    });
                }
                _ => {}
            }
        }

        let compat = vec![
            format!("pci{:04x},{:04x}", vendor_id, device_id),
            format!("pci-class-{:02x}{:02x}", class_code, subclass),
            String::from("pci-device"),
        ];

        dt.add_device(
            None,
            DeviceClass::Other,
            compat,
            resources,
            DeviceInitPriority::Regular,
        );
    } else if header_type == 0x01 {
        // PCI-to-PCI bridge
        let secondary_bus = ecam.read_u8(bdf, 0x19);
        let subordinate_bus = ecam.read_u8(bdf, 0x1A);

        debug!(
            "PCI: Bridge {} (Sec: {}, Sub: {})",
            bdf, secondary_bus, subordinate_bus
        );

        if secondary_bus > bdf.bus && secondary_bus <= subordinate_bus {
            scan_bus(ecam, secondary_bus, dt);
        }
    }
}
