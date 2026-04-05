use core::{
    arch::asm,
    fmt::Debug,
    sync::atomic::{AtomicU8, Ordering},
};

use aarch64_cpu::{
    asm::barrier::{self, dsb, isb},
    registers::{ReadWriteable, Readable, Writeable},
};

use super::{
    GICD_CTLR, GICD_TYPER, GICR_CTLR, GICR_WAKER, GicdRegisters, GicrRdRegisters, GicrSgiRegisters,
    InterruptController, InterruptInterface,
};

use self::registers::{icc_pmr_el1::ICC_PMR_EL1, icc_sre_el1::ICC_SRE_EL1};

pub mod registers;

static INIT_STATE: AtomicU8 = AtomicU8::new(0);

#[derive(Copy, Clone)]
pub struct GicV3<'a, I: InterruptInterface> {
    pub distributor: &'a GicdRegisters,
    pub redistributor_rd: &'a GicrRdRegisters,
    pub redistributor_sgi: &'a GicrSgiRegisters,
    pub iface: I,
}

impl<I: InterruptInterface> Debug for GicV3<'_, I> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GicV3").finish()
    }
}

impl<'a, I: InterruptInterface> GicV3<'a, I> {
    pub fn new(
        distributor: &'a GicdRegisters,
        redistributor_rd: &'a GicrRdRegisters,
        redistributor_sgi: &'a GicrSgiRegisters,
        iface: I,
    ) -> Self {
        Self {
            distributor,
            redistributor_rd,
            redistributor_sgi,
            iface,
        }
    }

    fn wait_for_distributor_rwp(&self) {
        dsb(barrier::SY);
        while self.distributor.CTLR.matches_all(GICD_CTLR::RWP::True) {
            core::hint::spin_loop();
        }
    }

    fn wait_for_redistributor_rwp(&self) {
        dsb(barrier::SY);
        while self.redistributor_rd.CTLR.matches_all(GICR_CTLR::RWP::True) {
            core::hint::spin_loop();
        }
    }
}

impl<'a, I: InterruptInterface> InterruptController for GicV3<'a, I> {
    type Error = GicError;

    fn init(&mut self) -> Result<(), Self::Error> {
        ICC_SRE_EL1.modify(ICC_SRE_EL1::SRE::Enabled);
        {
            let value = 0;
            unsafe { asm!("msr icc_bpr1_el1, {0:x}", in(reg) value) };
        }
        self.iface.enable_group1();
        self.iface.set_priority_mask(0xFF); // unmask every level
        isb(barrier::SY);

        match INIT_STATE.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => {
                self.distributor
                    .CTLR
                    .modify(GICD_CTLR::EnableGrp1::Disabled + GICD_CTLR::EnableGrp1A::Disabled);

                self.wait_for_distributor_rwp();

                self.distributor.CTLR.modify(GICD_CTLR::ARE_NS::Enabled);

                self.wait_for_distributor_rwp();

                // shared peripheral interrupts
                for i in 1..32 {
                    self.distributor.ICENABLER[i].set(0xFFFF_FFFF); // disable
                    self.distributor.ICPENDR[i].set(0xFFFF_FFFF); // clear pending
                    self.distributor.IGROUPR[i].set(0xFFFF_FFFF); // group 1 (non-secure)
                }

                for i in 32..1020 {
                    self.distributor.IPRIORITYR[i].set(0xA0); // default priority
                }

                self.distributor
                    .CTLR
                    .modify(GICD_CTLR::EnableGrp1::Enabled + GICD_CTLR::EnableGrp1A::Enabled);

                self.wait_for_distributor_rwp();

                INIT_STATE.store(2, Ordering::Release);
            }
            Err(_) => {
                while INIT_STATE.load(Ordering::Acquire) != 2 {
                    core::hint::spin_loop();
                }
            }
        }

        self.redistributor_rd
            .WAKER
            .modify(GICR_WAKER::ProcessorAsleep::Awake);
        dsb(barrier::SY);

        while self
            .redistributor_rd
            .WAKER
            .matches_all(GICR_WAKER::ProcessorAsleep::Sleep)
        {
            core::hint::spin_loop();
        }

        while self
            .redistributor_rd
            .WAKER
            .matches_all(GICR_WAKER::ChildrenAsleep::True)
        {
            core::hint::spin_loop();
        }

        // software generated interrupts and private peripheral interrupts
        self.redistributor_sgi.ICENABLER0.set(0xFFFF_FFFF); // disable SGI/PPI
        self.redistributor_sgi.ICPENDR0.set(0xFFFF_FFFF); // clear pending
        self.redistributor_sgi.IGROUPR0.set(0xFFFF_FFFF); // group 1 (non-secure)
        self.redistributor_sgi.IGRPMODR0.set(0xFFFF_FFFF);

        self.wait_for_redistributor_rwp();

        for i in 0..32 {
            self.redistributor_sgi.IPRIORITYR[i].set(0xA0); // default priority
        }

        isb(barrier::SY);

        Ok(())
    }

    fn enable_interrupt(&mut self, int_id: u32) -> Result<(), Self::Error> {
        if int_id < 32 {
            self.redistributor_sgi.ISENABLER0.set(1 << int_id);
            self.wait_for_redistributor_rwp();
        } else if int_id < 1020 {
            let reg_i = (int_id / 32) as usize;
            let bit = int_id % 32;
            self.distributor.ISENABLER[reg_i].set(1 << bit);
            self.wait_for_distributor_rwp();
        } else {
            return Err(GicError::InvalidInterruptId);
        }
        Ok(())
    }

    fn disable_interrupt(&mut self, int_id: u32) -> Result<(), Self::Error> {
        if int_id < 32 {
            self.redistributor_sgi.ICENABLER0.set(1 << int_id);
            self.wait_for_redistributor_rwp();
        } else if int_id < 1020 {
            let reg_i = (int_id / 32) as usize;
            let bit = int_id % 32;
            self.distributor.ICENABLER[reg_i].set(1 << bit);
            self.wait_for_distributor_rwp();
        } else {
            return Err(GicError::InvalidInterruptId);
        }
        Ok(())
    }

    fn acknowledge_interrupt(&mut self) -> Result<Option<u32>, Self::Error> {
        let int_id = self.iface.read_iar();

        // id 1023 is defined as spurious
        if int_id == 1023 {
            Ok(None)
        } else {
            Ok(Some(int_id))
        }
    }

    fn end_of_interrupt(&mut self, int_id: u32) -> Result<(), Self::Error> {
        if int_id < 1020 {
            self.iface.write_eoir(int_id);
            Ok(())
        } else {
            Err(GicError::InvalidInterruptId)
        }
    }

    fn set_priority(&mut self, int_id: u32, priority: u8) -> Result<(), Self::Error> {
        if int_id < 32 {
            self.redistributor_sgi.IPRIORITYR[int_id as usize].set(priority);
        } else if int_id < 1020 {
            self.distributor.IPRIORITYR[int_id as usize].set(priority);
        } else {
            return Err(GicError::InvalidInterruptId);
        }
        Ok(())
    }

    fn set_affinity(&mut self, int_id: u32, affinity: u64) -> Result<(), Self::Error> {
        if int_id < 32 {
            // SGIs and PPIs are private to a core
            return Err(GicError::NotSupported);
        } else if int_id >= 1020 {
            return Err(GicError::InvalidInterruptId);
        }

        self.distributor.IROUTER[int_id as usize].set(affinity);
        Ok(())
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum GicError {
    InvalidInterruptId,
    NotSupported,
}
