use alloc::vec::Vec;

use crate::{address::Bdf, ecam::Ecam};

#[derive(Debug, Clone)]
pub enum BarType {
    Memory32 {
        address: u32,
        size: u32,
        prefetchable: bool,
    },
    Memory64 {
        address: u64,
        size: u64,
        prefetchable: bool,
    },
    Io {
        address: u32,
        size: u32,
    },
}

const CMD_REG_OFFSET: u16 = 0x04;

/// guard to ensure PCI Command Register is restored upon drop, regardless of early returns
struct CommandRegGuard<'a> {
    ecam: &'a Ecam,
    bdf: Bdf,
    original_command: u16,
}

impl Drop for CommandRegGuard<'_> {
    fn drop(&mut self) {
        self.ecam
            .write_u16(self.bdf, CMD_REG_OFFSET, self.original_command);
    }
}

pub fn probe_bars(ecam: &Ecam, bdf: Bdf) -> Vec<BarType> {
    let mut bars = Vec::new();
    let mut bar_i = 0;

    let original_command = ecam.read_u16(bdf, CMD_REG_OFFSET);
    ecam.write_u16(bdf, CMD_REG_OFFSET, original_command & !0x03);

    let _guard = CommandRegGuard {
        ecam,
        bdf,
        original_command,
    };

    while bar_i < 6 {
        let offset = 0x10 + (bar_i * 4);
        let original_val = ecam.read_u32(bdf, offset);

        // probe size
        ecam.write_u32(bdf, offset, 0xFFFF_FFFF);
        let mask = ecam.read_u32(bdf, offset);
        ecam.write_u32(bdf, offset, original_val);

        let is_io = (original_val & 0x1) != 0;
        let type_bits = (original_val >> 1) & 0x3;

        if is_io {
            if mask == 0 || mask == !0 {
                bar_i += 1;
                continue;
            }

            let addr = original_val & 0xFFFF_FFFC;
            let size = (!(mask & 0xFFFF_FFFC)).wrapping_add(1);
            bars.push(BarType::Io {
                address: addr,
                size,
            });
            bar_i += 1;
        } else if type_bits == 0 {
            // Memory32 BAR
            if mask == 0 || mask == !0 {
                bar_i += 1;
                continue;
            }

            let prefetchable = ((original_val >> 3) & 0x1) != 0;
            let addr = original_val & 0xFFFF_FFF0;
            let size = (!(mask & 0xFFFF_FFF0)).wrapping_add(1);
            bars.push(BarType::Memory32 {
                address: addr,
                size,
                prefetchable,
            });
            bar_i += 1;
        } else if type_bits == 2 {
            // Memory64 BAR
            if bar_i >= 5 {
                log::warn!("{}: 64-bit BAR reported at invalid index {}", bdf, bar_i);
                bar_i += 1;
                continue;
            }
            let offset_high = offset + 4;
            let original_high = ecam.read_u32(bdf, offset_high);

            ecam.write_u32(bdf, offset_high, 0xFFFF_FFFF);
            let mask_high = ecam.read_u32(bdf, offset_high);
            ecam.write_u32(bdf, offset_high, original_high);

            let raw_mask = ((mask_high as u64) << 32) | (mask as u64 & 0xFFFF_FFF0);
            if raw_mask == 0 {
                bar_i += 2;
                continue;
            }

            let prefetchable = ((original_val >> 3) & 0x1) != 0;
            let addr = ((original_high as u64) << 32) | (original_val as u64 & 0xFFFF_FFF0);
            let size = (!raw_mask).wrapping_add(1);

            bars.push(BarType::Memory64 {
                address: addr,
                size,
                prefetchable,
            });
            bar_i += 2; // 64-bit BAR takes up two 32-bit regs
        } else {
            // reserved
            bar_i += 1;
        }
    }

    bars
}
