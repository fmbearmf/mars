use core::slice::Iter;

use crate::acpi::AcpiTableTrait;
use crate::impl_table;

use super::FromBytes;
use super::header::SdtHeader;
use hal::timer::TimerCapability;
use hax_lib::{attributes, ensures, opaque, requires};
use mars_getters::unaligned_getters;

impl_table! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

pub struct GtdtTimerIter<'a> {
    gtdt: &'a Gtdt,
    index: usize,
}

impl<'a> Iterator for GtdtTimerIter<'a> {
    type Item = (u32, TimerCapability);

    fn next(&mut self) -> Option<Self::Item> {
        let max_timers = if self.gtdt.header.rev() >= 3 { 5 } else { 4 };

        while self.index < max_timers {
            let i = self.index;
            self.index += 1;

            let timer = match i {
                0 => (
                    self.gtdt.secure_el1_gsiv(),
                    TimerCapability {
                        is_virtual: false,
                        is_hypervisor: false,
                        is_secure: true,
                    },
                ),
                1 => (
                    self.gtdt.ns_el1_gsiv(),
                    TimerCapability {
                        is_virtual: false,
                        is_hypervisor: false,
                        is_secure: false,
                    },
                ),
                2 => (
                    self.gtdt.virt_el1_gsiv(),
                    TimerCapability {
                        is_virtual: true,
                        is_hypervisor: false,
                        is_secure: false,
                    },
                ),
                3 => (
                    self.gtdt.ns_el2_gsiv(),
                    TimerCapability {
                        is_virtual: false,
                        is_hypervisor: true,
                        is_secure: false,
                    },
                ),

                _ => return None,
            };

            if timer.0 != 0 {
                return Some(timer);
            }
        }

        None
    }
}

impl<'a> IntoIterator for &'a Gtdt {
    type Item = (u32, TimerCapability);
    type IntoIter = GtdtTimerIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.timer_iter()
    }
}

impl Gtdt {
    fn timer_iter(&self) -> GtdtTimerIter {
        GtdtTimerIter {
            gtdt: self,
            index: 0,
        }
    }
}

#[attributes]
impl AcpiTableTrait for Gtdt {
    #[opaque]
    #[requires(slice.len() as usize >= core::mem::size_of::<Self>())]
    #[ensures(|result| result.is_ok())]
    fn safe_table_cast(slice: &'static [u8]) -> Result<&'static Self, &'static str> {
        let (reference, _) = Self::ref_from_prefix(slice).map_err(|_| "alignment/size error")?;
        Ok(reference)
    }
}
