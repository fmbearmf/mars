use core::{fmt::Debug, iter::FusedIterator, marker::PhantomData, ptr, slice, u32, u64};

use hax_lib::{attributes, ensures, opaque, requires};
use klib::{
    interrupt::{GicrRdRegisters, GicrSgiRegisters},
    vm::phys_addr_to_dmap,
};
use tock_registers::interfaces::Debuggable;

use crate::{acpi::AcpiTableTrait, impl_table};

use super::{SystemDescription, header::SdtHeader};
use mars_getters::{unaligned_getters, unaligned_getters_hax};

use super::FromBytes;

pub const MADT_GICC: u8 = 0x0B;
pub const MADT_GICD: u8 = 0x0C;
pub const MADT_GICR: u8 = 0x0E;
pub const MADT_ITS: u8 = 0x0F;

pub const GICR_FRAME_SIZE: usize = 0x002_0000; // 128kib
pub const GICR_SGI_OFFSET: usize = 0x001_0000;

impl_table! {
    #[derive(Debug, Clone, Copy)]
    pub struct Madt {
        pub header: SdtHeader,
        pub local_interrupt_ctrl: u32,
        pub flags: u32,
    }
}

#[attributes]
impl AcpiTableTrait for Madt {
    #[opaque]
    #[requires(slice.len() as usize >= core::mem::size_of::<Self>())]
    #[ensures(|result| result.is_ok())]
    fn safe_table_cast(slice: &'static [u8]) -> Result<&'static Self, &'static str> {
        let (reference, _) = Self::ref_from_prefix(slice).map_err(|_| "alignment/size error")?;
        Ok(reference)
    }
}

impl_table! {
    #[derive(Debug, Clone, Copy)]
    pub struct MadtEntryHeader {
        pub entry_type: u8,
        pub len: u8,
    }
}

impl_table! {
    #[derive(Debug, Clone, Copy)]
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
}

impl_table! {
    #[derive(Debug, Clone, Copy)]
    pub struct GicDistributor {
        pub header: MadtEntryHeader,
        pub reserved: u16,
        pub gic_id: u32,
        pub phys_base: u64,
        pub system_vector_base: u32,
        pub gic_version: u8,
        pub reserved2: [u8; 3],
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum GicrSplitError {
    UnalignedBase,
    UnalignedLength,
    BaseOOR,
    LengthOOR,
    Overflow,
}

#[derive(Debug)]
pub struct GicrFrameBlock<'a> {
    data: &'a [u8],
    count: usize,
}

#[derive(Copy, Clone)]
pub struct GicrFrame<'a> {
    pub rd: &'a GicrRdRegisters,
    pub sgi: &'a GicrSgiRegisters,
}

#[opaque]
impl<'a> Debug for GicrFrame<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "GicrFrame {{ rd: {:p}, sgi: {:p} }}", self.rd, self.sgi)
    }
}

impl<'a> GicrFrameBlock<'a> {
    #[requires(data.len() > 0)]
    #[ensures(|result| match result {
        Ok(r) => r.count > 0,
        Err(_) => true
    })]
    pub fn new(data: &'a [u8]) -> Result<Self, GicrSplitError> {
        let len = data.len();

        if len % GICR_FRAME_SIZE != 0 {
            return Err(GicrSplitError::UnalignedLength);
        }

        if len == 0 {
            unreachable!();
        }

        Ok(Self {
            data,
            count: len / GICR_FRAME_SIZE,
        })
    }

    pub fn len(&self) -> usize {
        self.count
    }

    #[requires(index < self.count)]
    pub fn get(&self, index: usize, virt: bool) -> Option<GicrFrame<'a>> {
        let offset = index.checked_mul(GICR_FRAME_SIZE)?;

        let frame_slice = self.data.get(offset..offset + GICR_FRAME_SIZE)?;

        let rd_slice = frame_slice.get(0..GICR_SGI_OFFSET)?;
        let sgi_slice = frame_slice.get(GICR_SGI_OFFSET..GICR_FRAME_SIZE)?;

        //let rd = GicrRdRegisters

        Some(GicrFrame {
            rd: rd_slice,
            sgi: sgi_slice,
        })
    }
}

impl_table! {
    #[derive(Debug, Clone, Copy)]
    pub struct GicRedistributor {
        pub header: MadtEntryHeader,
        pub flags: u8,
        pub reserved: u8,
        pub discovery_range_base: u64,
        pub discovery_range_len: u32,
    }
}

impl GicRedistributor {
    #[opaque]
    pub fn frames<'a>(&'a self) -> Result<GicrFrameBlock<'a>, GicrSplitError> {
        let base = self.discovery_range_base() as *const u8;
        let len = self.discovery_range_len() as usize;
        let slice = unsafe { slice::from_raw_parts(base, len) };
        GicrFrameBlock::new(slice)
    }
}

impl_table! {
    #[derive(Debug, Clone, Copy)]
    pub struct GicIts {
        pub header: MadtEntryHeader,
        pub flags: u8,
        pub reserved: u8,
        pub translation_id: u32,
        pub phys_base: u64,
        pub reserved2: u32,
    }
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
                let (gicd, _) = match GicDistributor::read_from_prefix(slice) {
                    Ok(gicd) => gicd,
                    Err(_) => unreachable!(),
                };
                gicd_phys_base = Some(gicd.phys_base());
            }
            MADT_GICR => {
                let (gicr, _) = match GicRedistributor::read_from_prefix(slice) {
                    Ok(gicr) => gicr,
                    Err(_) => unreachable!(),
                };
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
