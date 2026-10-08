use core::{
    cell::UnsafeCell,
    ptr::{NonNull, read_volatile},
};

#[repr(transparent)]
pub struct Volatile<T: Copy> {
    val: UnsafeCell<T>,
}

unsafe impl<T: Copy + Send> Send for Volatile<T> {}
unsafe impl<T: Copy + Sync> Sync for Volatile<T> {}

impl<T: Copy> Volatile<T> {
    #[inline]
    pub fn read(&self) -> T {
        unsafe { read_volatile(self.val.get()) }
    }

    #[inline]
    pub fn write(&self, val: T) {
        unsafe { core::ptr::write_volatile(self.val.get(), val) }
    }
}

pub struct XhciRegisters {
    cap: NonNull<CapabilityRegs>,
    op: NonNull<OperationalRegs>,
    rt: NonNull<RuntimeRegs>,
    db: NonNull<Volatile<u32>>,
}

unsafe impl Send for XhciRegisters {}

impl XhciRegisters {
    /// SAFETY: `base..base + mmio_size` must be mapped as xHCI MMIO.
    pub unsafe fn new(base: NonNull<u8>, mmio_size: usize) -> Result<Self, &'static str> {
        if mmio_size < size_of::<CapabilityRegs>() {
            return Err("xHCI MMIO range is smaller than its capability registers");
        }

        let base_ptr = base.as_ptr();
        let cap = base_ptr.cast::<CapabilityRegs>();
        let cap_len = unsafe { (*cap).cap_len.read() } as usize;
        let hcs_params1 = unsafe { (*cap).hcs_params1.read() };
        let max_slots = (hcs_params1 & 0xff) as usize;
        let max_ports = ((hcs_params1 >> 24) & 0xff) as usize;
        let rt_offset = (unsafe { (*cap).rts_off.read() } & !0x1f) as usize;
        let db_offset = (unsafe { (*cap).db_off.read() } & !0x03) as usize;

        if cap_len < size_of::<CapabilityRegs>() {
            return Err("invalid xHCI capability length");
        }
        let op_end = cap_len
            .checked_add(0x400)
            .and_then(|offset| offset.checked_add(max_ports.checked_mul(0x10)?))
            .ok_or("xHCI operational register range overflow")?;
        let runtime_end = rt_offset
            .checked_add(0x20 + size_of::<InterrupterRegisterSet>())
            .ok_or("xHCI runtime register range overflow")?;
        let doorbell_end = db_offset
            .checked_add(
                (max_slots + 1)
                    .checked_mul(size_of::<u32>())
                    .ok_or("xHCI doorbell range overflow")?,
            )
            .ok_or("xHCI doorbell range overflow")?;

        if op_end > mmio_size || runtime_end > mmio_size || doorbell_end > mmio_size {
            return Err("xHCI register offsets exceed the MMIO resource");
        }

        let op = unsafe { NonNull::new_unchecked(base_ptr.add(cap_len).cast()) };
        let rt = unsafe { NonNull::new_unchecked(base_ptr.add(rt_offset).cast()) };
        let db = unsafe { NonNull::new_unchecked(base_ptr.add(db_offset).cast()) };
        Ok(Self {
            cap: NonNull::new(cap).ok_or("null xHCI capability base")?,
            op,
            rt,
            db,
        })
    }

    #[inline]
    pub fn cap(&self) -> &CapabilityRegs {
        unsafe { self.cap.as_ref() }
    }

    #[inline]
    pub fn op(&self) -> &OperationalRegs {
        unsafe { self.op.as_ref() }
    }

    #[inline]
    pub fn rt(&self) -> &RuntimeRegs {
        unsafe { self.rt.as_ref() }
    }

    #[inline]
    pub fn ring_doorbell(&self, slot_id: u8, target: u8) {
        unsafe {
            let db_ptr = self.db.as_ptr().add(slot_id as usize);
            (*db_ptr).write(target as u32);
        }
    }
}

#[repr(C)]
pub struct CapabilityRegs {
    pub cap_len: Volatile<u8>,
    pub reserved: Volatile<u8>,

    pub hci_version: Volatile<u16>,
    pub hcs_params1: Volatile<u32>,
    pub hcs_params2: Volatile<u32>,
    pub hcs_params3: Volatile<u32>,
    pub hcc_params1: Volatile<u32>,

    pub db_off: Volatile<u32>,
    pub rts_off: Volatile<u32>,

    pub hcc_params2: Volatile<u32>,
}

#[repr(C)]
pub struct OperationalRegs {
    pub usb_cmd: Volatile<u32>,
    pub usb_sts: Volatile<u32>,
    pub page_size: Volatile<u32>,
    _res1: [u32; 2],
    pub dn_ctrl: Volatile<u32>,
    pub crcr: Volatile<u64>,
    _res2: [u32; 4],
    pub dc_baap: Volatile<u64>,
    pub config: Volatile<u32>,
}

#[repr(C)]
pub struct RuntimeRegs {
    pub mf_index: Volatile<u32>,
    _res1: [u32; 7],
    pub irs: [InterrupterRegisterSet; 1024],
}

#[repr(C)]
pub struct InterrupterRegisterSet {
    pub iman: Volatile<u32>,
    pub imod: Volatile<u32>,
    pub er_stsz: Volatile<u32>,
    pub resvd: Volatile<u32>,
    pub er_stba: Volatile<u64>,
    pub erdp: Volatile<u64>,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct ErstEntry {
    pub seg_base: u64,
    pub seg_size: u16,
    pub rsvd: [u16; 3],
}
