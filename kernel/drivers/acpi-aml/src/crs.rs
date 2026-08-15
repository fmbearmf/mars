use alloc::vec::Vec;
use klib::hardware::resource::Resource;

pub struct CrsIter<'a>(pub &'a [u8]);

impl<'a> Iterator for CrsIter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        let [tag, rest @ ..] = self.0 else {
            return None;
        };

        // end tag
        if *tag == 0x79 {
            return None;
        }

        let (len, data_start) = if (tag & 0x80) == 0 {
            // small descriptor
            ((tag & 0x07) as usize, 1)
        } else {
            let len_bytes = rest.get(0..2)?;
            (u16::from_le_bytes([len_bytes[0], len_bytes[1]]) as usize, 3)
        };

        let chunk = self.0.get(..data_start + len)?;
        self.0 = &self.0[data_start + len..];

        Some(chunk)
    }
}

pub trait DecodeCrs<'a> {
    fn into_rss(self) -> impl Iterator<Item = Resource>;
}

impl<'a> DecodeCrs<'a> for &[u8] {
    fn into_rss(self) -> impl Iterator<Item = Resource> {
        let mut mmio = None;
        let mut irqs = Vec::new();

        let tag = self[0];
        let is_large = (tag & 0x80) != 0;
        let item_type = if is_large {
            tag & 0x7F
        } else {
            (tag >> 3) & 0x0F
        };
        let data = if is_large {
            self.get(3..).unwrap_or(&[])
        } else {
            self.get(1..).unwrap_or(&[])
        };

        match (is_large, item_type) {
            // IRQ mask
            (false, 0x04) if data.len() >= 2 => {
                let mask = u16::from_le_bytes([data[0], data[1]]);
                irqs.extend(
                    (0..16)
                        .filter(|i| (mask & (1 << i)) != 0)
                        .map(Resource::Irq),
                );
            }
            // 32-bit memory range
            (true, 0x05) if data.len() >= 17 => {
                let base = u32::from_le_bytes(data[1..5].try_into().unwrap()) as usize;
                let len = u32::from_le_bytes(data[13..17].try_into().unwrap()) as usize;
                if len > 0 {
                    mmio = Some(Resource::Mmio {
                        range: base..base + len,
                    });
                }
            }
            // 32-bit fixed memory range
            (true, 0x06) if data.len() >= 9 => {
                let base = u32::from_le_bytes(data[1..5].try_into().unwrap()) as usize;
                let len = u32::from_le_bytes(data[5..9].try_into().unwrap()) as usize;
                if len > 0 {
                    mmio = Some(Resource::Mmio {
                        range: base..base + len,
                    });
                }
            }
            // dword address space
            (true, 0x07) if data.len() >= 23 && data[0] == 0 => {
                let base = u32::from_le_bytes(data[7..11].try_into().unwrap()) as usize;
                let len = u32::from_le_bytes(data[19..23].try_into().unwrap()) as usize;
                if len > 0 {
                    mmio = Some(Resource::Mmio {
                        range: base..base + len,
                    });
                }
            }
            // qword address space
            (true, 0x0A) if data.len() >= 43 && data[0] == 0 => {
                let base = u64::from_le_bytes(data[11..19].try_into().unwrap()) as usize;
                let len = u64::from_le_bytes(data[35..43].try_into().unwrap()) as usize;
                if len > 0 {
                    mmio = Some(Resource::Mmio {
                        range: base..base + len,
                    });
                }
            }
            // extended irq
            (true, 0x09) if data.len() >= 2 => {
                let count = data[1] as usize;
                let extracted = (0..count).filter_map(|i| {
                    let offset = 2 + i * 4;
                    let bytes = data.get(offset..offset + 4)?;
                    Some(Resource::Irq(u32::from_le_bytes(bytes.try_into().unwrap())))
                });
                irqs.extend(extracted);
            }
            _ => {}
        }

        mmio.into_iter().chain(irqs)
    }
}
