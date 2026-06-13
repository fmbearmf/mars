use klib::hardware::device::DeviceNode;
use phf::phf_map;

use crate::earlyinit::gicv3::gicv3_handler;

pub static DEVICE_TABLE: phf::Map<&str, fn(&mut DeviceNode)> = phf_map! {
    "arm,gic-v3" => gicv3_handler,
};
