extern crate alloc;

use core::mem::discriminant;

use super::resource::Resource;
use crate::cpu_interface::Mpidr;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Default)]
pub struct DeviceTree {
    /// arena of all devices
    nodes: Vec<DeviceNode>,
    /// root level devices
    roots: Vec<DeviceId>,
}

impl DeviceTree {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            roots: Vec::new(),
        }
    }

    pub fn add_device(
        &mut self,
        parent: Option<DeviceId>,
        class: DeviceClass,
        compatible: Vec<String>,
        resources: Vec<Resource>,
    ) -> DeviceId {
        let id = DeviceId(self.nodes.len());

        let node = DeviceNode {
            id,
            parent,
            class,
            compatible,
            resources,
            children: Vec::new(),
        };

        self.nodes.push(node);

        if let Some(parent_id) = parent {
            self.nodes[parent_id.0].children.push(id);
        } else {
            self.roots.push(id);
        }

        id
    }

    pub fn get(&self, id: DeviceId) -> Option<&DeviceNode> {
        self.nodes.get(id.0)
    }

    pub fn iter_class(&self, class: DeviceClass) -> impl Iterator<Item = &DeviceNode> {
        self.nodes
            .iter()
            .filter(move |node| discriminant(&node.class) == discriminant(&class))
    }
}

#[derive(Debug)]
pub struct DeviceNode {
    pub id: DeviceId,
    pub parent: Option<DeviceId>,
    pub class: DeviceClass,

    /// device identifiers
    pub compatible: Vec<String>,

    pub resources: Vec<Resource>,
    pub children: Vec<DeviceId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceClass {
    Cpu { id: Mpidr },
    Uart,
    Timer,
    InterruptController,
    PciHostBridge,
    Other,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct DeviceId(pub usize);
