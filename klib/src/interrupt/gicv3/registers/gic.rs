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
    /// thin u8 wrapper
    GicBitfield8, u8, {
        field Field => (
            offset: 0,
            size: 8,
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
        field PhysicalAddress => (
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

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    /// ITS Control
    GitsCtlr, u32, {
        /// global enable for ITS
        field Enabled => (
            offset: 0,
            size: 1,
            type: bool,
        );
        /// whether the ITS is "quiescent" (no pending cmds)
        /// RO
        field Quiescent => (
            offset: 31,
            size: 1,
            type: bool,
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    /// ITS type register
    GitsTyper, u64, {
        /// physical LPIs are supported
        field PhysicalLpiCapable => (
            offset: 0,
            size: 1,
            type: bool,
        );
        /// ITT entry size, minus 1
        field ITTEntrySize => (
            offset: 4,
            size: 4,
            type: u8,
        );
        /// number of EventID bits supported, minus 1
        field IDbits => (
            offset: 8,
            size: 5,
            type: u8,
        );
        /// number of DeviceId bits supported, minus 1
        field Devbits => (
            offset: 13,
            size: 5,
            type: u8,
        );
        /// physical target addr format.
        /// 0 = GICR_TYPER processor number, 1 = target distributor physical address
        field PTA => (
            offset: 19,
            size: 1,
            type: bool,
        );
        /// number of interrupt collections supported by ITS without external memory
        field HCC => (
            offset: 24,
            size: 8,
            type: u8,
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    /// ITS cmd queue BAR and size
    GitsCbasER, u64, {
        /// size of the command queue in 4k pages, minus 1
        field Size => (
            offset: 0,
            size: 8,
            type: u8,
        );
        /// shareability attribute of the cmd queue memory
        /// 0 = non-shareable, 1 = inner, 2 = outer, 3 = inner + outer
        field Shareability => (
            offset: 10,
            size: 2,
            type: u8,
        );
        /// physical addr of the command queue. must be 4k aligned
        field PhysicalAddress => (
            offset: 12,
            size: 40,
            type: u64,
        );
        field OuterCacheability => (
            offset: 53,
            size: 3,
            type: u8
        );
        field InnerCacheability => (
            offset: 59,
            size: 3,
            type: u8
        );
        /// the valid bit for the command queue
        field Valid => (
            offset: 63,
            size: 1,
            type: bool
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    /// ITS cmd queue write pointer
    GitsCwriter, u64, {
        /// offset from the command queue base to the next command to write.
        field Offset => (
            offset: 5,
            size: 15,
            type: u16,
        );
        /// retry flag (if STALLED set by hardware; otherwise no effect)
        field Retry => (
            offset: 0,
            size: 1,
            type: bool,
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    /// ITS cmd queue read pointer
    GitsCreadr, u64, {
        /// offset from the command queue base to the next command to be read by hardware
        field Offset => (
            offset: 5,
            size: 15,
            type: u16,
        );
        /// if the ITS is stalled (e.g. cmd sync failure)
        field Stalled => (
            offset: 0,
            size: 1,
            type: bool,
        );
    }
);

#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum GitsBaserType {
    #[default]
    Unallocated = 0b000,
    Device = 0b001,
    Vpe = 0b010,
    Collection = 0b100,
}

pub struct GitsBaserTypeValue(pub GitsBaserType);

impl<
    T: RegisterValue
        + Shr<usize, Output = T>
        + Shl<usize, Output = T>
        + BitAnd<Output = T>
        + BitOrAssign
        + PartialEq,
> FieldType<T> for GitsBaserTypeValue
{
    fn from_bits(bits: T) -> Self {
        let t = match bits {
            x if x == T::from(0b001) => GitsBaserType::Device,
            x if x == T::from(0b010) => GitsBaserType::Vpe,
            x if x == T::from(0b100) => GitsBaserType::Collection,
            _ => GitsBaserType::Unallocated,
        };
        GitsBaserTypeValue(t)
    }

    fn into_bits(self) -> T {
        let val = match self.0 {
            GitsBaserType::Device => 0b001,
            GitsBaserType::Vpe => 0b010,
            GitsBaserType::Collection => 0b100,
            GitsBaserType::Unallocated => 0b000,
        };
        T::from(val)
    }
}

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    /// ITS table BAR (GITS_BASER<0..8>)
    GitsBaser, u64, {
        /// pages allocated to the table, minus 1
        field Size => (
            offset: 0,
            size: 8,
            type: u8
        );
        /// page size of the table.
        /// 0 = 4k, 1 = 16k, 2 = 64k
        field PageSize => (
            offset: 8,
            size: 2,
            type: u8
        );
        /// 0 = non-shareable, 1 = inner, 2 = outer
        field Shareability => (
            offset: 10,
            size: 2,
            type: u8
        );
        /// physical address of the table, aligned to table page size
        field PhysicalAddress => (
            offset: 12,
            size: 36,
            type: u64
        );
        field OuterCacheability => (
            offset: 53,
            size: 3,
            type: u8
        );
        /// 1 = devices, 2 = vPEs, 4 = collections
        field Type => (
            offset: 56,
            size: 3,
            type: u8
        );
        field InnerCacheability => (
            offset: 59,
            size: 3,
            type: u8
        );
        /// whether a single flat table is used, or a 2-level table (where the 1st level has a list of descriptors)
        /// 0 = single. `Size` = number of pages used by ITS to store data associated with each table entry.
        /// 1 = 2-level. `Size` = number of pages which have an array of 64-bit descriptors to pages that are used to store the data associated with each table entry.
        field Indirect => (
            offset: 62,
            size: 1,
            type: bool
        );
        /// 0 = no memory is allocated for the table.
        /// 1 = memory is allocated for the table.
        field Valid => (
            offset: 63,
            size: 1,
            type: bool
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    LpiProp, u8, {
        field Enabled => (
            offset: 0,
            size: 1,
            type: bool
        );
        field Priority => (
            offset: 2,
            size: 6,
            type: u8
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    ItsCmdWord0, u64, {
        field Cmd => (
            offset: 0,
            size: 8,
            type: u8
        );
        field DeviceId => (
            offset: 32,
            size: 32,
            type: u32
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    ItsMapdWord1, u64, {
        field Size => (
            offset: 0,
            size: 5,
            type: u8
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    ItsMapdWord2, u64, {
        field IttAddr => (
            offset: 8,
            size: 44,
            type: u64
        );
        field Valid => (
            offset: 63,
            size: 1,
            type: bool
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    ItsMapcWord2, u64, {
        field Icid => (
            offset: 0,
            size: 16,
            type: u16
        );
        field RdBase => (
            offset: 16,
            size: 36,
            type: u64
        );
        field Valid => (
            offset: 63,
            size: 1,
            type: bool
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    ItsMaptiWord1, u64, {
        field EventId => (
            offset: 0,
            size: 32,
            type: u32
        );
        field pIntId => (
            offset: 32,
            size: 32,
            type: u32
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    ItsMaptiWord2, u64, {
        field Icid => (
            offset: 0,
            size: 16,
            type: u16
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    ItsInvallWord2, u64, {
        field Icid => (
            offset: 0,
            size: 16,
            type: u16
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    ItsSyncWord2, u64, {
        field RdBase => (
            offset: 16,
            size: 36,
            type: u64
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    ItsDiscardWord1, u64, {
        field EventId => (
            offset: 0,
            size: 32,
            type: u32
        );
    }
);

declare_register!(
    #[derive(Immutable, FromBytes, IntoBytes, KnownLayout)]
    ItsInvWord1, u64, {
        field EventId => (
            offset: 0,
            size: 32,
            type: u32
        );
    }
);
