use core::ops::{BitAnd, BitOrAssign, Shl, Shr};

use mars_models::{
    declare_register, declare_structs,
    memory::registers::{
        field::{FieldType, RegisterValue},
        volatile::{PureReadable, RPureReadWrite, Writeable},
    },
};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    /// the layout of GICD_CTLR *technically* varies depending on security state.
    /// these fields are present no matter what.
    GicdCtlr, u32, {
    /// group 1 non-secure (the only relevant group).
    field EnableGroup1 => (
        offset: 1,
        size: 1,
        type: bool,
    );
    /// affinity routing enable.
    field Are => (
        offset: 4,
        size: 1,
        type: bool
    );
    /// (if a) register write in progress.
    /// tracked writes:
    /// - GICD_CTLR group enable
    /// - GICR_CTLR ARE bit
    /// - GICD_ICENABLER<any>
    field RegisterWritePending => (
        offset: 31,
        size: 1,
        type: bool
    );
});

declare_register!(
    /// interrupt priority
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    GicdIPriorityr, u32, {
        field Priority0 => (
            offset: 0,
            size: 8,
            type: u8,
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    GicdTyper, u32, {
        /// the maximum SPI supported
        /// the max SPI IntID is 32(N+1) - 1 where N is the register value
        /// certsin IntIDs are reserved regardless
        field ITLinesNumber => (
            offset: 0,
            size: 5,
            type: u8,
        );
        /// # of cores that can be used when affinity routing isn't enabled, minus 1
        field CPUNumber => (
            offset: 5,
            size: 3,
            type: u8,
        );
        field SecurityExtension => (
            offset: 10,
            size: 1,
            type: bool,
        );
        field LPISupport => (
            offset: 17,
            size: 1,
            type: bool,
        );
    }
);

#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum GicdIcfgrValue {
    #[default]
    LevelSensitive = 0b00,
    EdgeTriggered = 0b10,
}

pub struct GicdIcfgrValues<const N: usize>(pub [GicdIcfgrValue; N]);

impl<
    T: RegisterValue
        + Shr<usize, Output = T>
        + Shl<usize, Output = T>
        + BitAnd<Output = T>
        + BitOrAssign
        + PartialEq,
    const N: usize,
> FieldType<T> for GicdIcfgrValues<N>
{
    fn from_bits(bits: T) -> Self {
        let mut result = [GicdIcfgrValue::default(); N];

        for i in 0..N {
            let shift = i * 2;
            let value = (bits >> shift) & T::from(0b11);

            result[i] = match value {
                x if x == T::from(0b10) => GicdIcfgrValue::EdgeTriggered,
                _ => GicdIcfgrValue::LevelSensitive,
            };
        }
        GicdIcfgrValues(result)
    }
    fn into_bits(self) -> T {
        let mut bits = T::ZERO;

        for i in 0..N {
            let shift = i * 2;

            let value = match self.0[i] {
                GicdIcfgrValue::LevelSensitive => 0b00,
                GicdIcfgrValue::EdgeTriggered => 0b10,
            };

            bits |= T::from(value) << shift;
        }
        bits
    }
}

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    /// set whether an interrupt is edge triggered or level sensitive
    GicIcfgr, u32, {
        field Interrupts => (
            offset: 0,
            size: 32,
            type: GicdIcfgrValues<32>,
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    /// thin u32 wrapper
    GicBitfield32, u32, {
        field Field => (
            offset: 0,
            size: 32,
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    /// thin u64 wrapper
    GicBitfield64, u64, {
        field Field => (
            offset: 0,
            size: 64,
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    /// if affinity routing is enabled, information for a particular SPI
    GicdIRouter, u64, {
        field Aff0 => (
            offset: 0,
            size: 8,
            type: u8,
        );
        field Aff1 => (
            offset: 8,
            size: 8,
            type: u8,
        );
        field Aff2 => (
            offset: 16,
            size: 8,
            type: u8,
        );
        /// false = route to the core with the MPIDR specified by the affinities.
        /// true = route to any core defined as a participaring node.
        field RouteToAny => (
            offset: 31,
            size: 1,
            type: bool,
        );
        field Aff3 => (
            offset: 32,
            size: 8,
            type: u8,
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    GicrWaker, u32, {
        field ProcessorSleep => (
            offset: 1,
            size: 1,
            type: bool,
        );
        field ChildrenAsleep => (
            offset: 2,
            size: 1,
            type: bool,
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    GicrCtlr, u32, {
    field EnableLPIs => (
        offset: 0,
        size: 1,
        type: bool,
    );
    field RegisterWritePending => (
        offset: 3,
        size: 1,
        type: bool,
    );
    /// when false, SPIs configured to use the 1 of N distribution model can select this core, if it's not asleep.
    /// when true, said SPIs can't select this core.
    field DisableProcessorSelection => (
        offset: 25,
        size: 1,
        type: bool,
    );
    /// whether upstream writes are still being sent to the distributor.
    field UpstreamWritePending => (
        offset: 31,
        size: 1,
        type: bool,
    );
});

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    GicrTyper, u64, {
        /// whether physical LPIs are supported
        field PhysicalLPISupport => (
            offset: 0,
            size: 1,
            type: bool,
        );
        /// whether direct injection of LPIs is supported
        field DirectLPISupport => (
            offset: 3,
            size: 1,
            type: bool,
        );
        /// whether this redistributor is the highest-numbered redistributor (ie last in the MMIO block)
        field LastRedistributor => (
            offset: 4,
            size: 1,
            type: bool,
        );
        /// unique ID for the core.
        field ProcessorNumber => (
            offset: 8,
            size: 16,
            type: u16,
        );
        /// the scope of the common LPI affinity group.
        /// 0 => all redistributors are in the same group.
        /// 1 => all redistributors with the same Aff3 are in the same group.
        /// 2 => all redistributors with the same Aff3.Aff2 are in the same group.
        /// 3 => all redistributors with the same Aff3.Aff2.Aff1 are in the same group.
        /// redistributors in the same group use the same LPI config table.
        field CommonLPIAffinity => (
            offset: 24,
            size: 2,
            type: u8,
        );
        /// the MPIDR of the core associated with this redistributor.
        /// bottom 8 bits = Aff0.
        /// next 8 bits = Aff1.
        /// next 8 bits = Aff2.
        /// next 8 bits = Aff3.
        field AffinityValue => (
            offset: 32,
            size: 32,
            type: u32,
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    GicrPropBar, u64, {
        /// bits of LPI INTIDs supported minus one
        field IDBits => (
            offset: 0,
            size: 5,
            type: u8
        );
        /// inner cacheability of memory accesses.
        /// 0 => device-nGnRnE.
        /// 1 => normal inner none-cacheable.
        /// 2 => normal inner cacheable read-alloc, write-through.
        /// 3 => normal inner cacheable read-alloc, write-back.
        /// 4 => normal inner cacheable write-alloc, write-through.
        /// 5 => normal inner cacheable write-alloc, write-back.
        /// 6 => normal inner cacheable read-alloc, write-alloc, write-through.
        /// 7 => normal inner cacheable read-alloc, write-alloc, write-back.
        field InnerCacheability => (
            offset: 7,
            size: 3,
            type: u8,
        );
        /// shareability of memory accesses.
        /// 0 => non-shareable.
        /// 1 => inner shareable.
        /// 2 => outer shareable.
        field Shareability => (
            offset: 10,
            size: 2,
            type: u8,
        );
        /// address of LPI config table.
        field Address => (
            offset: 12,
            size: 40,
            type: u64,
        );
        /// outer cacheability of memory accesses.
        /// 0 => type in InnerCacheability.
        /// 1 => normal outer non-cacheable.
        /// 2 => normal outer cacheable read-alloc, write-through.
        /// 3 => normal outer cacheable read-alloc, write-back.
        /// 4 => normal outer cacheable write-alloc, write-through.
        /// 5 => normal outer cacheable write-alloc, write-back.
        /// 6 => normal outer cacheable read-alloc, write-alloc, write-through.
        /// 7 => normal outer cacheable read-alloc, write-alloc, write-back.
        field OuterCacheability => (
            offset: 56,
            size: 3,
            type: u8,
        );
    }
);
