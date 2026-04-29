use aarch64_cpu_ext::registers::MPIDR_EL1::Aff0;
use tock_registers::register_bitfields;

pub mod icc_igrpen1_el1;
pub mod icc_pmr_el1;
pub mod icc_sre_el1;

register_bitfields![u32,
    pub GICD_CTLR [
        /// Enable Group 1 interrupts (non-secure)
        EnableGrp1      OFFSET(0) NUMBITS(1) [ Disabled = 0, Enabled = 1 ],
        /// Enable Group 1A interrupts
        EnableGrp1A     OFFSET(1) NUMBITS(1) [ Disabled = 0, Enabled = 1 ],
        /// Affinity Routing Enable non-secure
        ARE_NS          OFFSET(4) NUMBITS(1) [ Disabled = 0, Enabled = 1 ],
        /// Register Write Pending
        RWP             OFFSET(31) NUMBITS(1) [ False = 0, True = 1 ],
    ],

    pub GICR_CTLR [
        /// Enable LPIs
        EnableLPIs      OFFSET(0) NUMBITS(1) [ Disabled = 0, Enabled = 1 ],
        /// Register Write Pending
        RWP             OFFSET(3) NUMBITS(1) [ False = 0, True = 1 ],
    ],

    pub GICR_WAKER [
        ChildrenAsleep  OFFSET(2)  NUMBITS(1) [ True = 1, False = 0 ],
        ProcessorAsleep OFFSET(1)  NUMBITS(1) [ Sleep = 1, Awake = 0 ],
    ],

    pub GICD_TYPER [
        /// the maximum SPI supported for IntIDs 32-1019
        /// the max SPI IntID is 32(N+1) - 1 where N is the register value
        /// IntIDs 1020-1023 are reserved regardless
        ITLinesNumber       OFFSET(0) NUMBITS(5) [],
        /// # of cores that can be used when affinity routing isn't enabled, minus 1
        CPUNumber           OFFSET(5) NUMBITS(3) [],
        /// whether the GIC supports 2 security states
        SecurityExtn        OFFSET(10) NUMBITS(1) [],
        /// whether the GIC supports LPIs
        LPIS                OFFSET(17) NUMBITS(1) [],
    ],

    pub GICD_IIDR [
        /// JEP106 ID code of the designer of the distributor
        Implementer         OFFSET(0) NUMBITS(12) [],
        /// ...
        Revision            OFFSET(12) NUMBITS(4) [],
        /// product variant number
        Variant             OFFSET(16) NUMBITS(4) [],
        /// product ID
        ProductID           OFFSET(24) NUMBITS(8) [],
    ],

    pub GICD_INT [
        MASK                OFFSET(0) NUMBITS(32) [],
    ],

    pub GICD_ICFGR [
        INT0                OFFSET(0)  NUMBITS(2) [ Level = 0, Edge = 2 ],
        INT1                OFFSET(2)  NUMBITS(2) [ Level = 0, Edge = 2 ],
        INT2                OFFSET(4)  NUMBITS(2) [ Level = 0, Edge = 2 ],
        INT3                OFFSET(6)  NUMBITS(2) [ Level = 0, Edge = 2 ],
        INT4                OFFSET(8)  NUMBITS(2) [ Level = 0, Edge = 2 ],
        INT5                OFFSET(10) NUMBITS(2) [ Level = 0, Edge = 2 ],
        INT6                OFFSET(12) NUMBITS(2) [ Level = 0, Edge = 2 ],
        INT7                OFFSET(14) NUMBITS(2) [ Level = 0, Edge = 2 ],
        INT8                OFFSET(16) NUMBITS(2) [ Level = 0, Edge = 2 ],
        INT9                OFFSET(18) NUMBITS(2) [ Level = 0, Edge = 2 ],
        INT10               OFFSET(20) NUMBITS(2) [ Level = 0, Edge = 2 ],
        INT11               OFFSET(22) NUMBITS(2) [ Level = 0, Edge = 2 ],
        INT12               OFFSET(24) NUMBITS(2) [ Level = 0, Edge = 2 ],
        INT13               OFFSET(26) NUMBITS(2) [ Level = 0, Edge = 2 ],
        INT14               OFFSET(28) NUMBITS(2) [ Level = 0, Edge = 2 ],
        INT15               OFFSET(30) NUMBITS(2) [ Level = 0, Edge = 2 ],
    ],
];

register_bitfields![u64,
    pub GICD_IROUTER [
        Aff0        OFFSET(0) NUMBITS(8) [],
        Aff1        OFFSET(8) NUMBITS(8) [],
        Aff2        OFFSET(16) NUMBITS(8) [],
        InterruptRoutingMode OFFSET(31) NUMBITS(1) [
            AffinityCore = 0,
            AnyCore = 1,
        ],
        Aff3        OFFSET(32) NUMBITS(8) [],
    ],
];
