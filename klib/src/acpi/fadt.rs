use core::ptr;

use super::GenericAddress;
use super::header::SdtHeader;
use getters::unaligned_getters;

#[repr(C, packed)]
#[unaligned_getters]
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
    pub arm_boot_arch: u16,
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
