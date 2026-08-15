use core::fmt::Write;

use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

use crate::parser::AmlParser;

pub trait AmlValueExt {
    fn as_string(&self) -> Option<String>;
    fn as_u64(&self) -> Option<u64>;
}

#[derive(Debug, Clone)]
pub enum AmlValue<'a> {
    Zero,
    One,
    Ones,
    Integer(u64),
    String(&'a str),
    NamePath(AmlNamePath<'a>),
    Buffer(&'a [u8]),
    Package(Vec<AmlValue<'a>>),
}

impl<'a> AmlValueExt for AmlValue<'a> {
    fn as_string(&self) -> Option<String> {
        match self {
            AmlValue::String(s) => Some(s.to_string()),
            AmlValue::NamePath(p) => Some(p.to_string()),
            AmlValue::Integer(v) => {
                let m1 = ((v >> 26) & 0x1F) as u8;
                let m2 = ((v >> 21) & 0x1F) as u8;
                let m3 = ((v >> 16) & 0x1F) as u8;
                let p = (v & 0xFFFF) as u16;
                Some(format!(
                    "{}{}{}{:04X}",
                    (0x40 + m1) as char,
                    (0x40 + m2) as char,
                    (0x40 + m3) as char,
                    p
                ))
            }
            _ => None,
        }
    }

    fn as_u64(&self) -> Option<u64> {
        match self {
            AmlValue::Zero => Some(0),
            AmlValue::One => Some(1),
            AmlValue::Ones => Some(u64::MAX),
            AmlValue::Integer(v) => Some(*v),
            AmlValue::String(s) => s.parse().ok(),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub enum AmlTerm<'a> {
    Scope {
        name: AmlNamePath<'a>,
        contents: AmlParser<'a>,
    },
    Device {
        name: AmlNamePath<'a>,
        contents: AmlParser<'a>,
    },
    Name {
        name: AmlNamePath<'a>,
        value: AmlValue<'a>,
    },
    Method {
        name: AmlNamePath<'a>,
        flags: u8,
        code: &'a [u8],
    },
    OpRegion {
        name: AmlNamePath<'a>,
    },
    Field,
    UnsupportedOpcode(u8),
}

#[derive(Debug, Copy, Clone)]
pub struct AmlNamePath<'a> {
    pub raw: &'a [u8],
}

impl<'a> core::fmt::Display for AmlNamePath<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut bytes = self.raw;

        while let Some(&b) = bytes.first() {
            match b {
                b'\\' | b'^' => {
                    f.write_str(if b == b'\\' { "\\" } else { "^" })?;
                    bytes = &bytes[1..];
                }
                _ => break,
            }
        }

        if bytes.is_empty() {
            return Ok(());
        }

        let prefix = bytes[0];
        let (num_segs, seg_bytes) = match prefix {
            0x00 => (0, &[][..]),
            0x2E => (2, &bytes[1..]), // DualNamePrefix
            0x2F => {
                if bytes.len() >= 2 {
                    (bytes[1] as usize, &bytes[2..])
                } else {
                    (0, &[][..])
                }
            }
            _ => (1, bytes), // one NameSeg
        };

        for i in 0..num_segs {
            let start = i * 4;
            let end = start + 4;

            if seg_bytes.len() < end {
                if i > 0 {
                    f.write_char('.')?;
                }
                f.write_str("????")?;
                break;
            }

            let seg = &seg_bytes[start..end];

            if i > 0 {
                f.write_char('.')?;
            }

            let mut actual_len = 4;
            while actual_len > 0 && seg[actual_len - 1] == b'_' {
                if actual_len == 1 {
                    break;
                }

                actual_len -= 1;
            }

            for &b in &seg[..actual_len] {
                if b.is_ascii_graphic() || b == b' ' || b == b'_' {
                    f.write_str(core::str::from_utf8(&[b]).unwrap_or("?"))?;
                } else {
                    f.write_str("?")?;
                }
            }
        }

        Ok(())
    }
}
