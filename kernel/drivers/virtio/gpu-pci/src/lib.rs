#![no_std]

use core::{
    alloc::Layout,
    ops::BitOr,
    ptr::{addr_of, addr_of_mut, read_volatile, write_volatile},
    sync::atomic::{Ordering, fence},
};

use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
use alloc::{
    alloc::{alloc_zeroed, dealloc},
    boxed::Box,
    vec::Vec,
};
use klib::{
    allocator_support::KernelAddressTranslator,
    hardware::{
        device::{DeviceNode, IrqFn},
        resource::Resource,
    },
    pm::page::mapper::AddressTranslator,
    vm::{MAIR_DEVICE_INDEX, PAGE_SIZE, align_up, user::address_space::KERNEL_ADDRESS_SPACE},
};
use mars_pcie_driver::{
    address::Bdf,
    bar::{BarType, probe_bars},
    capability::standard::{StandardCapIter, StandardCapabilityId},
    ecam::Ecam,
};

extern crate alloc;

pub mod constants {
    pub const VIRTIO_VENDOR_ID: u16 = 0x1AF4;
    pub const VIRTIO_GPU_DEVICE_ID: u16 = 0x1050;
}

pub fn handle(node: &DeviceNode, _enable_irq: IrqFn, _disable_irq: IrqFn) {
    if let Err(e) = try_handle(node) {
        use log::*;
        error!("virtio-gpu-pci driver error: {:?}", e);
    }
}

fn try_handle(node: &DeviceNode) -> Result<(), VirtioError> {
    let pci_info = node
        .resources
        .iter()
        .find_map(|r| match r {
            Resource::PciEcam {
                segment,
                bus,
                device,
                function,
                ecam_phys_base,
                ecam_start_bus,
                ecam_end_bus,
            } => Some((
                *segment,
                *bus,
                *device,
                *function,
                *ecam_phys_base,
                *ecam_start_bus,
                *ecam_end_bus,
            )),
            _ => None,
        })
        .ok_or(VirtioError::NotPciDevice)?;

    let (segment, bus, device, func, phys_base, start_bus, end_bus) = pci_info;

    let ecam = Ecam::new(phys_base, segment, start_bus, end_bus);
    let bdf = Bdf::new(segment, bus, device, func);

    let vendor_id = ecam.read_u16(bdf, 0x00);
    let device_id = ecam.read_u16(bdf, 0x02);

    if vendor_id != constants::VIRTIO_VENDOR_ID || device_id != constants::VIRTIO_GPU_DEVICE_ID {
        return Err(VirtioError::WrongDeviceType);
    }

    let gpu = VirtioGpu::init(&ecam, bdf)?;

    let _gpu_ref = Box::leak(Box::new(gpu));
    log::info!("VirtIO GPU h/w initialized on BDF {}", bdf);

    Ok(())
}

#[derive(Debug)]
pub enum VirtioError {
    NotPciDevice,
    WrongDeviceType,
    CapabilityNotFound(VirtioCapType),
    InvalidBar,
    FeatureNegotiationFailed,
    ZeroQueueSize,
    InsufficientDescriptors,
    AllocationFailed,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum VirtioCapType {
    CommonCfg = 1,
    NotifyCfg = 2,
    IsrCfg = 3,
    DeviceCfg = 4,
    PciCfg = 5,
}

impl TryFrom<u8> for VirtioCapType {
    type Error = ();
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::CommonCfg),
            2 => Ok(Self::NotifyCfg),
            3 => Ok(Self::IsrCfg),
            4 => Ok(Self::DeviceCfg),
            5 => Ok(Self::PciCfg),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(transparent)]
pub struct DeviceStatus(u8);

impl DeviceStatus {
    pub const ACKNOWLEDGE: Self = Self(0b1);
    pub const DRIVER: Self = Self(0b10);
    pub const DRIVER_OK: Self = Self(0b100);
    pub const FEATURES_OK: Self = Self(0b1000);
    pub const DEVICE_NEEDS_RESET: Self = Self(0b0100_0000);
    pub const FAILED: Self = Self(0b1000_0000);

    pub fn empty() -> Self {
        Self(0b0)
    }
    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl BitOr for DeviceStatus {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(transparent)]
pub struct DescFlags(u16);

impl DescFlags {
    pub const NEXT: Self = Self(1);
    pub const WRITE: Self = Self(2);
}

#[repr(C)]
pub struct VirtioPciCommonCfg {
    pub device_feature_select: u32,
    pub device_feature: u32,
    pub driver_feature_select: u32,
    pub driver_feature: u32,
    pub msix_config: u16,
    pub num_queues: u16,
    pub device_status: u8,
    pub config_generation: u8,
    pub queue_select: u16,
    pub queue_size: u16,
    pub queue_msix_vector: u16,
    pub queue_enable: u16,
    pub queue_notify_off: u16,

    pub queue_desc_low: u32,
    pub queue_desc_high: u32,
    pub queue_driver_low: u32,
    pub queue_driver_high: u32,
    pub queue_device_low: u32,
    pub queue_device_high: u32,
}

#[derive(Debug, Copy, Clone)]
pub struct VirtioPciCap {
    pub cap_vndr: u8,
    pub cap_next: u8,
    pub cap_len: u8,
    pub cfg_type: VirtioCapType,
    pub bar: u8,
    pub id: u8,
    pub offset: u32,
    pub length: u32,
    pub notify_off_multiplier: u32,
}

#[repr(C, align(16))]
pub struct VirtqDesc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

#[repr(C)]
pub struct VirtioGpuCtrlHdr {
    pub type_: u32,
    pub flags: u32,
    pub fence_id: u64,
    pub ctx_id: u32,
    pub ring_index: u8,
    pub padding: [u8; 3],
}

pub fn parse_virtio_caps(ecam: &Ecam, bdf: Bdf) -> Vec<VirtioPciCap> {
    StandardCapIter::new(ecam, bdf)
        .filter(|(id, _)| matches!(id, StandardCapabilityId::VendorSpecific))
        .filter_map(|(_, offset)| {
            let offset = offset as u16;

            let cap_len = ecam.read_u8(bdf, offset + 2);
            let cfg_type = VirtioCapType::try_from(ecam.read_u8(bdf, offset + 3)).ok()?;
            let bar = ecam.read_u8(bdf, offset + 4);
            let id_reg = ecam.read_u8(bdf, offset + 5);
            let mem_offset = ecam.read_u32(bdf, offset + 8);
            let length = ecam.read_u32(bdf, offset + 12);

            let notify_off_multiplier = if cfg_type == VirtioCapType::NotifyCfg && cap_len >= 20 {
                ecam.read_u32(bdf, offset + 16)
            } else {
                0
            };

            Some(VirtioPciCap {
                cap_vndr: ecam.read_u8(bdf, offset),
                cap_next: ecam.read_u8(bdf, offset + 1),
                cap_len,
                cfg_type,
                bar,
                id: id_reg,
                offset: mem_offset,
                length,
                notify_off_multiplier,
            })
        })
        .collect()
}

pub struct VirtioPciTransport {
    common_cfg: *mut VirtioPciCommonCfg,
    notify_cfg: *mut u8,
    isr_cfg: *mut u8,
    device_cfg: *mut u8,
    notify_off_multiplier: u32,
}

// SAFETY: all ptrs map to seperate MMIO segments exclusively owned by this instance
unsafe impl Send for VirtioPciTransport {}

impl VirtioPciTransport {
    pub fn new(ecam: &Ecam, bdf: Bdf) -> Result<Self, VirtioError> {
        let caps = parse_virtio_caps(ecam, bdf);
        let bars = probe_bars(ecam, bdf);

        let get_ptr = |target_cfg_type| -> Result<(*mut u8, VirtioPciCap), VirtioError> {
            let cap = caps
                .iter()
                .find(|c| c.cfg_type == target_cfg_type)
                .ok_or(VirtioError::CapabilityNotFound(target_cfg_type))?;

            let bar = bars
                .get(cap.bar as usize)
                .and_then(|opt| opt.as_ref())
                .ok_or(VirtioError::InvalidBar)?;

            let (address, bar_size) = match bar {
                BarType::Memory32 { address, size, .. } => (*address as usize, *size as usize),
                BarType::Memory64 { address, size, .. } => (*address as usize, *size as usize),
                _ => return Err(VirtioError::InvalidBar),
            };

            let base_vaddr = KernelAddressTranslator.phys_to_dmap(address) as usize;
            let vaddr =
                KernelAddressTranslator.phys_to_dmap(address + cap.offset as usize) as *mut u8;

            let mut cursor = KERNEL_ADDRESS_SPACE.lock(core::range::Range::from(
                base_vaddr..(base_vaddr + bar_size),
            ));

            cursor.map(
                address as _,
                AccessPermission::PrivilegedReadWrite,
                Shareability::OuterShareable,
                true,
                true,
                MAIR_DEVICE_INDEX,
            );

            Ok((vaddr, *cap))
        };

        let common_cfg = get_ptr(VirtioCapType::CommonCfg)?
            .0
            .cast::<VirtioPciCommonCfg>();
        let (notify_cfg, notify_cap) = get_ptr(VirtioCapType::NotifyCfg)?;
        let isr_cfg = get_ptr(VirtioCapType::IsrCfg)?.0;
        let device_cfg = get_ptr(VirtioCapType::DeviceCfg)
            .map(|(ptr, _)| ptr)
            .unwrap_or(core::ptr::null_mut());

        ecam.enable_memory_space(bdf);
        ecam.enable_bus_master(bdf);

        Ok(Self {
            common_cfg,
            notify_cfg,
            isr_cfg,
            device_cfg,
            notify_off_multiplier: notify_cap.notify_off_multiplier,
        })
    }

    pub fn read_device_status(&self) -> DeviceStatus {
        let val = unsafe { read_volatile(addr_of!((*self.common_cfg).device_status)) };
        DeviceStatus(val)
    }

    pub fn write_device_status(&self, status: DeviceStatus) {
        unsafe { write_volatile(addr_of_mut!((*self.common_cfg).device_status), status.0) }
    }

    pub fn read_device_feature(&self, select: u32) -> u32 {
        unsafe {
            write_volatile(
                addr_of_mut!((*self.common_cfg).device_feature_select),
                select,
            );
            read_volatile(addr_of!((*self.common_cfg).device_feature))
        }
    }

    pub fn write_driver_feature(&self, select: u32, feature: u32) {
        unsafe {
            write_volatile(
                addr_of_mut!((*self.common_cfg).driver_feature_select),
                select,
            );
            write_volatile(addr_of_mut!((*self.common_cfg).driver_feature), feature);
        }
    }

    pub fn set_queue_select(&self, index: u16) {
        unsafe { write_volatile(addr_of_mut!((*self.common_cfg).queue_select), index) }
    }

    pub fn get_queue_size(&self) -> u16 {
        unsafe { read_volatile(addr_of!((*self.common_cfg).queue_size)) }
    }

    pub fn set_queue_size(&self, size: u16) {
        unsafe { write_volatile(addr_of_mut!((*self.common_cfg).queue_size), size) }
    }

    pub fn set_queue_desc(&self, addr: u64) {
        unsafe {
            write_volatile(addr_of_mut!((*self.common_cfg).queue_desc_low), addr as u32);
            write_volatile(
                addr_of_mut!((*self.common_cfg).queue_desc_high),
                (addr >> 32) as u32,
            );
        }
    }

    pub fn set_queue_driver(&self, addr: u64) {
        unsafe {
            write_volatile(
                addr_of_mut!((*self.common_cfg).queue_driver_low),
                addr as u32,
            );
            write_volatile(
                addr_of_mut!((*self.common_cfg).queue_driver_high),
                (addr >> 32) as u32,
            );
        }
    }

    pub fn set_queue_device(&self, addr: u64) {
        unsafe {
            write_volatile(
                addr_of_mut!((*self.common_cfg).queue_device_low),
                addr as u32,
            );
            write_volatile(
                addr_of_mut!((*self.common_cfg).queue_device_high),
                (addr >> 32) as u32,
            );
        }
    }

    pub fn enable_queue(&self) {
        unsafe { write_volatile(addr_of_mut!((*self.common_cfg).queue_enable), 1) }
    }

    pub fn get_queue_notify_off(&self) -> u16 {
        unsafe { read_volatile(addr_of!((*self.common_cfg).queue_notify_off)) }
    }

    pub fn notify_queue(&self, queue_index: u16, notify_off: u16) {
        let offset = (notify_off as u32) * self.notify_off_multiplier;
        unsafe {
            write_volatile(
                self.notify_cfg.add(offset as usize) as *mut u16,
                queue_index,
            )
        }
    }
}

struct DmaAlloc {
    ptr: *mut u8,
    layout: Layout,
}

impl DmaAlloc {
    fn new_zeroed(size: usize) -> Result<Self, VirtioError> {
        let aligned_size = align_up(size, PAGE_SIZE);
        let layout = Layout::from_size_align(aligned_size, PAGE_SIZE)
            .map_err(|_| VirtioError::AllocationFailed)?;

        // SAFETY: size > 0 and layout is aligned
        let ptr = unsafe { alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err(VirtioError::AllocationFailed);
        }

        Ok(Self { ptr, layout })
    }

    fn as_mut_ptr(&self) -> *mut u8 {
        self.ptr
    }
    fn phys_addr(&self) -> u64 {
        KernelAddressTranslator.dmap_to_phys(self.ptr) as u64
    }
}

impl Drop for DmaAlloc {
    fn drop(&mut self) {
        // SAFETY: struct allocated by `new_zeroed`
        unsafe {
            dealloc(self.ptr, self.layout);
        }
    }
}

pub struct VirtQueue {
    pub index: u16,
    pub size: u16,
    pub notify_off: u16,

    desc_alloc: DmaAlloc,
    avail_alloc: DmaAlloc,
    used_alloc: DmaAlloc,

    pub free_head: u16,
    pub num_free: u16,
    pub last_used_index: u16,
}

unsafe impl Send for VirtQueue {}

impl VirtQueue {
    pub fn new(transport: &VirtioPciTransport, index: u16) -> Result<Self, VirtioError> {
        transport.set_queue_select(index);
        let size = transport.get_queue_size();

        if size == 0 {
            return Err(VirtioError::ZeroQueueSize);
        }

        let notify_off = transport.get_queue_notify_off();

        let desc_alloc = DmaAlloc::new_zeroed(16 * size as usize)?;
        let avail_alloc = DmaAlloc::new_zeroed(6 + 2 * size as usize)?;
        let used_alloc = DmaAlloc::new_zeroed(6 + 8 * size as usize)?;

        let desc_slice = unsafe {
            core::slice::from_raw_parts_mut(
                desc_alloc.as_mut_ptr().cast::<VirtqDesc>(),
                size as usize,
            )
        };

        for i in 0..(size - 1) {
            desc_slice[i as usize].next = i + 1;
        }
        desc_slice[(size - 1) as usize].next = 0;

        // disable interrupts; VRING_AVAIL_F_NO_INTERRUPT
        unsafe {
            write_volatile(avail_alloc.as_mut_ptr().cast::<u16>(), 1);
        };

        transport.set_queue_desc(desc_alloc.phys_addr());
        transport.set_queue_driver(avail_alloc.phys_addr());
        transport.set_queue_device(used_alloc.phys_addr());
        transport.set_queue_size(size);
        transport.enable_queue();

        Ok(Self {
            index,
            size,
            notify_off,
            desc_alloc,
            avail_alloc,
            used_alloc,
            free_head: 0,
            num_free: size,
            last_used_index: 0,
        })
    }

    pub fn send_gpu_command(
        &mut self,
        transport: &VirtioPciTransport,
        req_phys: u64,
        req_len: u32,
        resp_phys: u64,
        resp_len: u32,
    ) -> Result<(), VirtioError> {
        if self.num_free < 2 {
            return Err(VirtioError::InsufficientDescriptors);
        }

        let desc_slice = unsafe {
            core::slice::from_raw_parts_mut(
                self.desc_alloc.as_mut_ptr().cast::<VirtqDesc>(),
                self.size as usize,
            )
        };

        let d1_i = self.free_head;
        let d2_i = desc_slice[d1_i as usize].next;

        self.free_head = desc_slice[d2_i as usize].next;
        self.num_free -= 2;

        // cmd descriptor
        desc_slice[d1_i as usize].addr = req_phys;
        desc_slice[d1_i as usize].len = req_len;
        desc_slice[d1_i as usize].flags = DescFlags::NEXT.0;
        desc_slice[d1_i as usize].next = d2_i;

        // response descriptor
        desc_slice[d2_i as usize].addr = resp_phys;
        desc_slice[d2_i as usize].len = resp_len;
        desc_slice[d2_i as usize].flags = DescFlags::WRITE.0;

        unsafe {
            let avail_ptr = self.avail_alloc.as_mut_ptr();

            let avail_i = read_volatile(avail_ptr.add(2).cast::<u16>());
            let ring_offset = (avail_i % self.size) as usize;

            write_volatile(avail_ptr.add(4 + ring_offset * 2).cast::<u16>(), d1_i);

            fence(Ordering::SeqCst);
            write_volatile(avail_ptr.add(2).cast::<u16>(), avail_i.wrapping_add(1));
            fence(Ordering::SeqCst);
        }

        transport.notify_queue(self.index, self.notify_off);

        // spin for completion
        loop {
            // used_ptr[2..4] = used index
            let used_i =
                unsafe { read_volatile(self.used_alloc.as_mut_ptr().add(2).cast::<u16>()) };

            if self.last_used_index != used_i {
                fence(Ordering::Acquire);

                self.last_used_index = self.last_used_index.wrapping_add(1);

                // recover rss
                desc_slice[d2_i as usize].next = self.free_head;
                self.free_head = d1_i;
                self.num_free += 2;
                break;
            }
            core::hint::spin_loop();
        }

        Ok(())
    }
}

pub struct VirtioGpu {
    pub transport: VirtioPciTransport,
    pub controlq: VirtQueue,
    pub cursorq: VirtQueue,
}

impl VirtioGpu {
    pub fn init(ecam: &Ecam, bdf: Bdf) -> Result<Self, VirtioError> {
        let transport = VirtioPciTransport::new(ecam, bdf)?;

        transport.write_device_status(DeviceStatus::empty());
        while transport.read_device_status() != DeviceStatus::empty() {
            core::hint::spin_loop();
        }

        // ack phase
        transport.write_device_status(DeviceStatus::ACKNOWLEDGE);

        let status = transport.read_device_status();
        transport.write_device_status(status | DeviceStatus::DRIVER);

        // feature negotiation
        let features = transport.read_device_feature(1);
        transport.write_driver_feature(1, features | 1);

        let status = transport.read_device_status();
        transport.write_device_status(status | DeviceStatus::FEATURES_OK);

        if !transport
            .read_device_status()
            .contains(DeviceStatus::FEATURES_OK)
        {
            return Err(VirtioError::FeatureNegotiationFailed);
        }

        let controlq = VirtQueue::new(&transport, 0)?;
        let cursorq = VirtQueue::new(&transport, 1)?;

        let status = transport.read_device_status();
        transport.write_device_status(status | DeviceStatus::DRIVER_OK);

        Ok(Self {
            transport,
            controlq,
            cursorq,
        })
    }
}
