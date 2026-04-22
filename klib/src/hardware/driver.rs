use alloc::{boxed::Box, string::String, vec::Vec};

use crate::hardware::device::DeviceTree;

use super::device::{Device, DeviceNode};

#[derive(Debug, PartialEq, Eq, Hash)]
pub enum DriverError {
    MissingResources,
    Io,
    Incompatible,
}

pub struct DriverDescriptor {
    pub name: &'static str,
    pub compatible: &'static [&'static str],
    pub probe: fn(node: &DeviceNode) -> Result<Box<dyn Device>, DriverError>,
}

pub struct DriverManager {
    drivers: &'static [DriverDescriptor],
    instances: Vec<Box<dyn Device>>,
}

impl DriverManager {
    pub const fn new(drivers: &'static [DriverDescriptor]) -> Self {
        Self {
            drivers,
            instances: Vec::new(),
        }
    }

    pub fn bind_drivers(&mut self, dt: &DeviceTree) {
        for node in &dt.nodes {
            let driver = self.drivers.iter().find(|drv| {
                drv.compatible
                    .iter()
                    .any(|&c| node.compatible.contains(&String::from(c)))
            });

            if let Some(drv) = driver {
                match (drv.probe)(node) {
                    Ok(device) => self.instances.push(device),
                    Err(_) => panic!("probe fail"),
                }
            }
        }
    }
}

#[macro_export]
macro_rules! register_drivers {
    ( [ $( $desc:expr ),* $(,)? ] ) => {
        static DRIVERS: &[&'static $crate::hardware::driver::DriverDescriptor] = &[
            $( &$desc ),*
        ];
    };
}
