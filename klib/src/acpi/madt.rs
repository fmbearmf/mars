use core::{fmt::Debug, iter::FusedIterator, marker::PhantomData, ptr, slice, u32, u64};

use tock_registers::interfaces::Debuggable;

use super::{
    super::{
        interrupt::{GicdRegisters, GicrRdRegisters, GicrSgiRegisters},
        vm::phys_addr_to_dmap,
    },
    SystemDescription,
    header::SdtHeader,
};
use getters::unaligned_getters;

use zerocopy::{FromBytes, Immutable, Unaligned};

pub const MADT_GICC: u8 = 0x0B;
pub const MADT_GICD: u8 = 0x0C;
pub const MADT_GICR: u8 = 0x0E;
pub const MADT_ITS: u8 = 0x0F;

pub const GICR_FRAME_SIZE: usize = 0x002_0000; // 128kib
pub const GICR_SGI_OFFSET: usize = 0x001_0000;

#[repr(C, packed)]
#[unaligned_getters]
#[derive(FromBytes, Immutable, Unaligned)]
pub struct Madt {
    pub header: SdtHeader,
    pub local_interrupt_ctrl: u32,
    pub flags: u32,
}

#[repr(C, packed)]
#[unaligned_getters]
#[derive(Debug, Copy, Clone, FromBytes, Immutable, Unaligned)]
pub struct MadtEntryHeader {
    pub entry_type: u8,
    pub len: u8,
}

#[repr(C, packed)]
#[unaligned_getters]
#[derive(Debug, Copy, Clone, FromBytes, Immutable, Unaligned)]
pub struct GicCpuInterface {
    pub header: MadtEntryHeader,
    pub reserved: u16,
    pub cpu_interface_number: u32,
    pub acpi_cpu_uid: u32,
    pub flags: u32,
    pub parking_proto_ver: u32,
    pub perf_interrupt_gsiv: u32,
    pub parked_addr: u64,
    pub phys_base_addr: u64,
    pub gicv: u64,
    pub gich: u64,
    pub vgic_maint_int: u32,
    pub gicr_base_addr: u64,
    pub mpidr: u64,
    pub efficiency_class: u8,
    pub reserved2: [u8; 3],
}

#[repr(C, packed)]
#[unaligned_getters]
#[derive(Debug, Copy, Clone, FromBytes, Immutable, Unaligned)]
pub struct GicDistributor {
    pub header: MadtEntryHeader,
    pub reserved: u16,
    pub gic_id: u32,
    pub phys_base: u64,
    pub system_vector_base: u32,
    pub gic_version: u8,
    pub reserved2: [u8; 3],
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum GicrSplitError {
    UnalignedBase { base: *mut u8 },
    UnalignedLength { len: u64 },
    BaseOOR,
    LengthOOR,
    Overflow,
}

#[derive(Debug)]
pub struct GicrFrameBlock<'a> {
    base: *mut u8,
    count: usize,
    _lifetime: PhantomData<&'a mut [u8]>,
}

#[derive(Copy, Clone)]
pub struct GicrFrame<'a> {
    pub rd: &'a GicrRdRegisters,
    pub sgi: &'a GicrSgiRegisters,
}

impl<'a> Debug for GicrFrame<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "GicrFrame {{ rd: {:p}, sgi: {:p} }}", self.rd, self.sgi)
    }
}

impl<'a> GicrFrameBlock<'a> {
    pub unsafe fn new(entry: &GicRedistributor, virt: bool) -> Result<Self, GicrSplitError> {
        let base = if virt {
            phys_addr_to_dmap(entry.discovery_range_base())
        } else {
            entry.discovery_range_base()
        };
        let len = entry.discovery_range_len() as u64;

        if base % GICR_FRAME_SIZE as u64 != 0 {
            return Err(GicrSplitError::UnalignedBase {
                base: base as *mut u8,
            });
        }

        if len % GICR_FRAME_SIZE as u64 != 0 {
            return Err(GicrSplitError::UnalignedLength { len });
        }

        let base = base as usize;
        let len = len as usize;

        let count = len / GICR_FRAME_SIZE;

        base.checked_add(len).ok_or(GicrSplitError::Overflow)?;

        Ok(Self {
            base: base as *mut u8,
            count,
            _lifetime: PhantomData,
        })
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn get(&self, index: usize, virt: bool) -> Option<GicrFrame<'a>> {
        if index >= self.count {
            return None;
        }

        let frame_base = self.base as usize + index * GICR_FRAME_SIZE;

        let rd = unsafe { &*(frame_base as *const GicrRdRegisters) };

        let sgi = unsafe { &*((frame_base + GICR_SGI_OFFSET) as *const GicrSgiRegisters) };

        Some(GicrFrame { rd, sgi })
    }
}

#[repr(C, packed)]
#[unaligned_getters]
#[derive(Debug, Copy, Clone, FromBytes, Immutable, Unaligned)]
pub struct GicRedistributor {
    pub header: MadtEntryHeader,
    pub flags: u8,
    pub reserved: u8,
    pub discovery_range_base: u64,
    pub discovery_range_len: u32,
}

impl GicRedistributor {
    pub unsafe fn frames<'a>(&'a self, virt: bool) -> Result<GicrFrameBlock<'a>, GicrSplitError> {
        unsafe { GicrFrameBlock::new(self, virt) }
    }
}

#[repr(C, packed)]
#[unaligned_getters]
#[derive(Debug, Copy, Clone, FromBytes, Immutable, Unaligned)]
pub struct GicIts {
    pub header: MadtEntryHeader,
    pub flags: u8,
    pub reserved: u8,
    pub translation_id: u32,
    pub phys_base: u64,
    pub reserved2: u32,
}

pub struct MadtIter {
    ptr: *const u8,
    end: *const u8,
}

impl Iterator for MadtIter {
    type Item = (u8, &'static [u8]);
    fn next(&mut self) -> Option<Self::Item> {
        if self.ptr >= self.end {
            return None;
        }

        unsafe {
            let entry_type = ptr::read_unaligned(self.ptr);
            let len = ptr::read_unaligned(self.ptr.add(1));
            if len < 2 {
                return None;
            }
            let slice = slice::from_raw_parts(self.ptr, len as usize);
            self.ptr = self.ptr.add(len as usize);
            Some((entry_type, slice))
        }
    }
}

impl Madt {
    pub fn entries(&self) -> MadtIter {
        unsafe {
            let start = self.header.data_ptr().add(8);
            let end = (self as *const _ as *const u8).add(self.header.len() as usize);
            MadtIter { ptr: start, end }
        }
    }
}

pub fn parse_gic_addresses(sys: &SystemDescription) -> Option<(u64, u64)> {
    let madt = sys.madt?;

    let mut gicd_phys_base = None;
    let mut gicr_phys_base = None;

    for (type_, slice) in madt.entries() {
        match type_ {
            MADT_GICD => {
                let (gicd, _) = GicDistributor::read_from_prefix(slice).unwrap();
                gicd_phys_base = Some(gicd.phys_base());
            }
            MADT_GICR => {
                let (gicr, _) = GicRedistributor::read_from_prefix(slice).unwrap();
                gicr_phys_base = Some(gicr.discovery_range_base());
            }
            _ => {}
        }
    }

    match (gicd_phys_base, gicr_phys_base) {
        (Some(d), Some(r)) => Some((d, r)),
        _ => None,
    }
}
