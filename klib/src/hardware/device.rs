use core::mem::discriminant;

use super::irq::CallbackError;
use super::resource::Resource;
use crate::cpu_interface::CpuTopologyId;
use alloc::string::String;
use alloc::vec::Vec;

pub type IrqFn = fn(&Resource) -> Result<(), CallbackError>;
pub type DeviceHandler = fn(&DeviceNode, IrqFn, IrqFn);

pub trait Device: Send + Sync {
    fn shutdown(&self);
}

#[derive(Debug, Default)]
pub struct DeviceTree {
    /// arena of all devices
    pub nodes: Vec<DeviceNode>,
    /// root level devices
    pub roots: Vec<DeviceId>,
}

impl DeviceTree {
    pub const fn new() -> Self {
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
        priority: DeviceInitPriority,
    ) -> DeviceId {
        let id = DeviceId(self.nodes.len());

        let node = DeviceNode {
            id,
            parent,
            priority,
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

#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum DeviceInitPriority {
    #[default]
    Regular,
    Fundamental,
}

#[derive(Debug)]
pub struct DeviceNode {
    pub id: DeviceId,
    pub priority: DeviceInitPriority,
    pub parent: Option<DeviceId>,
    pub class: DeviceClass,

    /// device identifiers
    pub compatible: Vec<String>,

    pub resources: Vec<Resource>,
    pub children: Vec<DeviceId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceClass {
    Cpu { id: CpuTopologyId, acpi_uid: u32 },
    Uart,
    Timer,
    // hopefully you have less than 4 billion redistributors
    GicV3 { redistributor_count: u32 },
    PciHostBridge,
    Other,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct DeviceId(pub usize);
