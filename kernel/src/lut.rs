use crate::earlyinit::gicv3;
use klib::hardware::device::DeviceHandler;
use mars_generic_timer_driver as gt;
use phf::phf_map;

#[derive(Debug)]
pub enum DeviceCallback {
    /// call on one core
    Once(DeviceHandler),
    /// call on one core, and call a different function on every other core
    EveryCore((DeviceHandler, DeviceHandler)),
}

pub static DEVICE_TABLE: phf::Map<&str, DeviceCallback> = phf_map! {
    "arm,gic-v3" => DeviceCallback::EveryCore((gicv3::handle, gicv3::secondary_handle)),
    "arm,armv8-timer" => DeviceCallback::EveryCore((gt::handle, gt::secondary_handle)),
};
