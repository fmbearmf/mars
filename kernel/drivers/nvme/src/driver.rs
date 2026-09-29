use alloc::{boxed::Box, format, sync::Arc, vec::Vec};

use klib::{
    block::{self, BlockDevice, registry::register_hardware_device},
    hardware::{
        device::{DeviceNode, IrqFn},
        resource::{DmaRemapping, Resource, pci_bar_range},
    },
    scheduler::GLOBAL_SCHEDULER,
    sync::{FairSpinlock, SleepingMutex},
};
use mars_pcie_driver::{
    address::Bdf,
    capability::standard::{StandardCapIter, StandardCapabilityId},
    ecam::Ecam,
};

use crate::{
    controller::{Controller, NamespaceInfo},
    registers::Registers,
};

type ControllerHandle = Arc<SleepingMutex<'static, Controller>>;
static CONTROLLERS: FairSpinlock<Vec<(Bdf, ControllerHandle)>> = FairSpinlock::new(Vec::new());

pub fn handle(device: &DeviceNode, _enable_irq: IrqFn, _disable_irq: IrqFn) {
    if let Err(error) = attach(device) {
        log::error!("nvme: attachment failed: {error}");
    }
}

fn attach(device: &DeviceNode) -> Result<(), &'static str> {
    let (ecam, bdf) = device
        .resources
        .iter()
        .find_map(|resource| match resource {
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
        .ok_or("missing PCI configuration resource")?;
    if CONTROLLERS
        .lock()
        .iter()
        .any(|(registered, _)| *registered == bdf)
    {
        return Ok(());
    }
    if ecam.read_u8(bdf, 0x0b) != 1 || ecam.read_u8(bdf, 0x0a) != 8 || ecam.read_u8(bdf, 9) != 2 {
        return Err("unsupported PCI storage interface");
    }
    let bar = pci_bar_range(&device.resources, 0).ok_or("missing NVMe BAR0")?;
    let size = bar
        .end
        .checked_sub(bar.start)
        .filter(|size| *size >= 0x1008)
        .ok_or("invalid NVMe BAR0")?;
    // keep DMA disabled until the reset has completed and the new queues are owned
    let command = ecam.read_u16(bdf, 4);
    ecam.write_u16(bdf, 4, (command | (1 << 1) | (1 << 10)) & !(1 << 2));
    let header = unsafe { Registers::map(bar.start, 0x1000)? };
    let cap = header.read64(0);
    let stride = 4usize << ((cap >> 32) & 0xf);
    let register_bytes = 0x1000 + 3 * stride + 4;
    if register_bytes > size {
        return Err("BAR0 omits I/O queue doorbells");
    }
    let regs = unsafe { Registers::map(bar.start, register_bytes)? };
    let address_bits = dma_address_bits(device)?;

    for (kind, offset) in StandardCapIter::new(&ecam, bdf) {
        let offset = u16::from(offset) + 2;
        match kind {
            StandardCapabilityId::Msi => {
                ecam.write_u16(bdf, offset, ecam.read_u16(bdf, offset) & !1)
            }
            StandardCapabilityId::MsiX => ecam.write_u16(
                bdf,
                offset,
                (ecam.read_u16(bdf, offset) | (1 << 14)) & !(1 << 15),
            ),
            _ => {}
        }
    }
    let (controller, namespaces) =
        match Controller::initialize(regs, address_bits, || ecam.enable_bus_master(bdf)) {
            Ok(value) => value,
            Err(error) => {
                ecam.write_u16(bdf, 4, ecam.read_u16(bdf, 4) & !(1 << 2));
                return Err(error);
            }
        };
    let controller = Arc::new(SleepingMutex::new(controller));
    let index = {
        let mut controllers = CONTROLLERS.lock();
        let index = controllers.len();
        controllers.push((bdf, Arc::clone(&controller)));
        index
    };
    for info in namespaces {
        let name = format!("nvme{index}n{}", info.nsid);
        log::info!(
            "{name}: {} blocks of {} bytes on {bdf}",
            info.block_count,
            info.block_size
        );
        let namespace = Namespace {
            controller: Arc::clone(&controller),
            info,
        };
        let _handle = register_hardware_device(name, Box::new(namespace));
        #[cfg(feature = "self-test")]
        crate::self_test::run(&_handle, &controller, info.nsid);
    }
    Ok(())
}

fn dma_address_bits(device: &DeviceNode) -> Result<u8, &'static str> {
    let (bits, remapping) = device
        .resources
        .iter()
        .find_map(|resource| match resource {
            Resource::PciDmaTopology {
                address_bits,
                remapping,
                ..
            } => Some((*address_bits, *remapping)),
            _ => None,
        })
        .ok_or("PCI DMA topology unresolved")?;
    match remapping {
        DmaRemapping::NoIommu => Ok(bits),
        DmaRemapping::ArmSmmuV3 { base, .. } => {
            let base = usize::try_from(base).map_err(|_| "SMMU address too wide")?;
            // read-only.
            let regs = unsafe { Registers::map(base, 0x10000)? };
            let cr0 = regs.read32(0x20);
            let ack = regs.read32(0x24);
            let gbpa = regs.read32(0x44);
            if cr0 & 1 != 0 || ack & 1 != 0 || gbpa & ((1 << 31) | (1 << 20)) != 0 {
                return Err("SMMUv3 requires a DMA translation domain");
            }
            Ok(bits)
        }
        DmaRemapping::ArmSmmu { .. } => Err("SMMUv1/v2 DMA translation is not implemented"),
    }
}

struct Namespace {
    controller: ControllerHandle,
    info: NamespaceInfo,
}

impl BlockDevice for Namespace {
    fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> block::Result<()> {
        self.controller.lock(&GLOBAL_SCHEDULER).read_blocks(
            self.info.nsid,
            self.info.block_size,
            self.info.block_count,
            lba,
            buf,
        )
    }
    fn write_blocks(&mut self, lba: u64, buf: &[u8]) -> block::Result<()> {
        self.controller.lock(&GLOBAL_SCHEDULER).write_blocks(
            self.info.nsid,
            self.info.block_size,
            self.info.block_count,
            lba,
            buf,
        )
    }
    fn flush(&mut self) -> block::Result<()> {
        self.controller
            .lock(&GLOBAL_SCHEDULER)
            .flush(self.info.nsid)
    }
    fn block_size(&self) -> usize {
        self.info.block_size
    }
    fn block_count(&self) -> u64 {
        self.info.block_count
    }
}
