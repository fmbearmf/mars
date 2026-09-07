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
    /// SAFETY: base must be a valid pointer to MMIO
    pub unsafe fn new(base: NonNull<u8>) -> Self {
        let base_ptr = base.as_ptr();
        let cap = base_ptr.cast::<CapabilityRegs>();

        let cap_len = unsafe { (*cap).cap_len.read() };
        let rt_offset = unsafe { (*cap).rts_off.read() } & !0x1F; // 32-byte alignment
        let db_offset = unsafe { (*cap).db_off.read() } & !0x03; // 4-byte alignment

        unsafe {
            Self {
                cap: NonNull::new_unchecked(cap),
                op: NonNull::new_unchecked(base_ptr.add(cap_len as usize).cast()),
                rt: NonNull::new_unchecked(base_ptr.add(rt_offset as usize).cast()),
                db: NonNull::new_unchecked(base_ptr.add(db_offset as usize).cast()),
            }
        }
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
