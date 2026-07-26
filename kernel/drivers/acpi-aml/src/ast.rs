use core::fmt::Write;

use crate::parser::AmlParser;

#[derive(Debug, Copy, Clone)]
pub enum AmlValue<'a> {
    Zero,
    One,
    Ones,
    Integer(u64),
    String(&'a str),
    NamePath(AmlNamePath<'a>),
    Buffer(&'a [u8]),
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

            for &b in seg {
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
