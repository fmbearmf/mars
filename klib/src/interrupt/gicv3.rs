use aarch64_cpu::registers::{ReadWriteable, Readable, Writeable};

use super::{
    GICD_CTLR, GICR_WAKER, GicdRegisters, GicrRdRegisters, GicrSgiRegisters, InterruptController,
    InterruptInterface,
};

pub struct GicV3<'a, I: InterruptInterface> {
    distributor: &'a GicdRegisters,
    redistributor_rd: &'a GicrRdRegisters,
    redistributor_sgi: &'a GicrSgiRegisters,
    iface: I,
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
}

impl<'a, I: InterruptInterface> InterruptController for GicV3<'a, I> {
    type Error = GicError;

    fn init(&mut self) -> Result<(), Self::Error> {
        self.distributor
            .CTLR
            .modify(GICD_CTLR::ENABLE_G1A::Disable + GICD_CTLR::ENABLE_G1::Disable);

        self.distributor.CTLR.modify(GICD_CTLR::ARE_NS::Enable);

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
            .modify(GICD_CTLR::ENABLE_G1A::Enable + GICD_CTLR::ENABLE_G1::Enable);

        self.redistributor_rd
            .WAKER
            .modify(GICR_WAKER::ProcessorAsleep::Awake);
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

        for i in 0..32 {
            self.redistributor_sgi.IPRIORITYR[i].set(0xA0); // default priority
        }

        self.iface.set_priority_mask(0xFF); // unmask every level
        self.iface.enable_group1();

        Ok(())
    }

    fn enable_interrupt(&mut self, int_id: u32) -> Result<(), Self::Error> {
        if int_id < 32 {
            self.redistributor_sgi.ISENABLER0.set(1 << int_id);
        } else if int_id < 1020 {
            let reg_i = (int_id / 32) as usize;
            let bit = int_id % 32;
            self.distributor.ISENABLER[reg_i].set(1 << bit);
        } else {
            return Err(GicError::InvalidInterruptId);
        }
        Ok(())
    }

    fn disable_interrupt(&mut self, int_id: u32) -> Result<(), Self::Error> {
        if int_id < 32 {
            self.redistributor_sgi.ICENABLER0.set(1 << int_id);
        } else if int_id < 1020 {
            let reg_i = (int_id / 32) as usize;
            let bit = int_id % 32;
            self.distributor.ICENABLER[reg_i].set(1 << bit);
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
            Ok(Some((int_id)))
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
