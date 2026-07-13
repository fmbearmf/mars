use core::ptr;

use crate::acpi::AcpiTableTrait;
use crate::impl_table;

use super::{FromBytes, Immutable, IntoBytes};
use super::{GenericAddress, header::SdtHeader};
use hax_lib::{attributes, opaque};

impl_table! {
    #[derive(Debug, Clone, Copy)]
    pub struct Fadt {
        pub header: SdtHeader,
        pub firmware_ctrl: u32,
        pub dsdt: u32,
        //
        pub interrupt_model: u8, // reserved
        //
        pub preferred_pm_profile: u8,
        pub sci_int: u16,
        pub smmi_cmd: u32,
        pub acpi_enable: u8,
        pub acpi_disable: u8,
        pub s4bios_req: u8,
        pub pstate_control: u8,
        pub pm1a_ev_blk: u32,
        pub pm1b_ev_blk: u32,
        pub pm1a_ctrl_blk: u32,
        pub pm1b_ctrl_blk: u32,
        pub pm2_ctrl_blk: u32,
        pub pm_timer_blk: u32,
        pub gpe0_blk: u32,
        pub gpe1_blk: u32,
        pub pm1_ev_len: u8,
        pub pm1_ctrl_len: u8,
        pub pm2_ctrl_len: u8,
        pub pm_timer_len: u8,
        pub gpe0_len: u8,
        pub gpe1_len: u8,
        pub gpe1_base: u8,
        pub cstate_control: u8,
        pub worst_c2_latency: u16,
        pub worst_c3_latency: u16,
        pub flush_size: u16,
        pub flush_stride: u16,
        pub duty_offset: u8,
        pub duty_width: u8,
        pub day_alarm: u8,
        pub month_alarm: u8,
        pub century: u8,
        //
        pub boot_arch_flags: u16,
        pub reserved: u8,
        pub flags: u32,
        //
        pub reset_reg: GenericAddress,
        pub reset_value: u8,
        pub arm_boot_arch: ArmBootArchFlags,
        pub minor_version: u8,
        // 64-bit
        pub x_fw_ctrl: u64,
        pub x_dsdt: u64,
        pub x_pm1a_ev_blk: GenericAddress,
        pub x_pm1b_ev_blk: GenericAddress,
        pub x_pm1a_ctrl_blk: GenericAddress,
        pub x_pm1b_ctrl_blk: GenericAddress,
        pub x_pm2_ctrl_blk: GenericAddress,
        pub x_pm_timer_blk: GenericAddress,
        pub x_gpe0_blk: GenericAddress,
        pub x_gpe1_blk: GenericAddress,
        //
        pub sleep_ctrl: GenericAddress,
        pub sleep_status: GenericAddress,
        //
        pub hypervisor_id: u64,
    }
}

#[attributes]
impl AcpiTableTrait for Fadt {
    #[opaque]
    #[requires(slice.len() as usize >= core::mem::size_of::<Self>())]
    #[ensures(|result| result.is_ok())]
    fn safe_table_cast(slice: &'static [u8]) -> Result<&'static Self, &'static str> {
        let (reference, _) = Self::ref_from_prefix(slice).map_err(|_| "alignment/size error")?;
        Ok(reference)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, FromBytes, IntoBytes, Immutable)]
#[repr(transparent)]
pub struct ArmBootArchFlags(u16);

impl ArmBootArchFlags {
    /// does the platform implement PSCI?
    pub const PSCI_COMPLIANT: u16 = 1 << 0;

    /// use HVC instead of SMC for PSCI?
    pub const PSCI_USE_HVC: u16 = 1 << 1;

    pub const fn is_psci_compliant(self) -> bool {
        (self.0 & Self::PSCI_COMPLIANT) != 0
    }

    pub const fn psci_use_hvc(self) -> bool {
        (self.0 & Self::PSCI_USE_HVC) != 0
    }
}
