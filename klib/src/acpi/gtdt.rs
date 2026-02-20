use super::header::SdtHeader;

#[repr(C, packed)]
pub struct Gtdt {
    pub header: SdtHeader,
    pub cnt_control_base: u64,
    pub reserved: u32,
    pub secure_el1_gsiv: u32,
    pub secure_el1_flags: u32,
    pub ns_el1_gsiv: u32,
    pub ns_el1_flags: u32,
    pub virt_el1_gsiv: u32,
    pub virt_el1_flags: u32,
    pub ns_el2_gsiv: u32,
    pub ns_el2_flags: u32,
    pub cnt_read_base: u64,
    pub platform_timer_count: u32,
    pub platform_timer_offset: u32,
}
