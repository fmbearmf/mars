use alloc::vec::Vec;

use crate::ast::{AmlNamePath, AmlTerm, AmlValue};

#[derive(Debug, Copy, Clone)]
pub struct AmlParser<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> AmlParser<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.cursor >= self.bytes.len()
    }

    pub fn parse_next(&mut self) -> Result<Option<AmlTerm<'a>>, &'static str> {
        if self.is_empty() {
            return Ok(None);
        }

        let opcode = self.read_u8()?;

        match opcode {
            // AliasOp
            0x06 => {
                self.read_name_path()?;
                self.read_name_path()?;
                Ok(Some(AmlTerm::UnsupportedOpcode(opcode)))
            }
            // ScopeOp
            0x10 => {
                let pkg_start = self.cursor;
                let pkg_len = self.read_pkg_length()?;
                let _pkg_header_bytes = self.cursor - pkg_start;

                let name = self.read_name_path()?;

                let consumed_bytes = self.cursor - pkg_start;
                let body_len = pkg_len
                    .checked_sub(consumed_bytes)
                    .ok_or("ScopeOp PkgLength underflowed")?;

                let body_bytes = self.take_bytes(body_len)?;

                Ok(Some(AmlTerm::Scope {
                    name,
                    contents: AmlParser::new(body_bytes),
                }))
            }
            // NameOp
            0x08 => {
                let name = self.read_name_path()?;
                let value = self.parse_data_object()?;
                Ok(Some(AmlTerm::Name { name, value }))
            }
            // MethodOp
            0x14 => {
                let pkg_start = self.cursor;
                let pkg_len = self.read_pkg_length()?;

                let name = self.read_name_path()?;
                let flags = self.read_u8()?;

                let consumed_bytes = self.cursor - pkg_start;
                let code_len = pkg_len
                    .checked_sub(consumed_bytes)
                    .ok_or("MethodOp PkgLength underflowed")?;

                let code = self.take_bytes(code_len)?;

                Ok(Some(AmlTerm::Method { name, flags, code }))
            }
            // ExternalOp
            0x15 => {
                self.read_name_path()?;
                let object_type = self.read_u8()?;
                if object_type == 0x08 {
                    self.read_u8()?;
                }
                Ok(Some(AmlTerm::UnsupportedOpcode(opcode)))
            }
            // ExtOp Prefix
            0x5B => {
                let ext_opc = self.read_u8()?;
                match ext_opc {
                    // OpRegionOp
                    0x80 => {
                        let name = self.read_name_path()?;
                        let _region_space = self.read_u8()?;
                        let _offset = self.parse_data_object()?;
                        let _length = self.parse_data_object()?;

                        Ok(Some(AmlTerm::OpRegion { name }))
                    }

                    // FieldOp
                    0x81 => {
                        let pkg_start = self.cursor;
                        let pkg_len = self.read_pkg_length()?;
                        let _region_name = self.read_name_path()?;
                        let _field_flags = self.read_u8()?;

                        let consumed_bytes = self.cursor - pkg_start;
                        let body_len = pkg_len
                            .checked_sub(consumed_bytes)
                            .ok_or("FieldOp PkgLength underflowed")?;

                        let _field_data = self.take_bytes(body_len)?;

                        Ok(Some(AmlTerm::Field))
                    }

                    // DeviceOp
                    0x82 => {
                        let pkg_start = self.cursor;
                        let pkg_len = self.read_pkg_length()?;

                        let name = self.read_name_path()?;

                        let consumed_bytes = self.cursor - pkg_start;
                        let body_len = pkg_len
                            .checked_sub(consumed_bytes)
                            .ok_or("DeviceOp PkgLength underflowed")?;

                        let body_bytes = self.take_bytes(body_len)?;

                        Ok(Some(AmlTerm::Device {
                            name,
                            contents: AmlParser::new(body_bytes),
                        }))
                    }
                    _ => Ok(Some(AmlTerm::UnsupportedOpcode(ext_opc))),
                }
            }
            op => Ok(Some(AmlTerm::UnsupportedOpcode(op))),
        }
    }

    pub fn parse_data_object(&mut self) -> Result<AmlValue<'a>, &'static str> {
        let op = self.read_u8()?;
        match op {
            // self-explanatory
            0x00 => Ok(AmlValue::Zero),
            0x01 => Ok(AmlValue::One),
            0xFF => Ok(AmlValue::Ones),

            // int consts
            0x0A => Ok(AmlValue::Integer(self.read_u8()? as u64)),
            0x0B => Ok(AmlValue::Integer(self.read_u16_le()? as u64)),
            0x0C => Ok(AmlValue::Integer(self.read_u32_le()? as u64)),
            0x0E => Ok(AmlValue::Integer(self.read_u64_le()?)),

            // string prefix
            0x0D => {
                let start = self.cursor;
                while self.read_u8()? != 0x00 {}
                let str_bytes = &self.bytes[start..self.cursor - 1];
                let string =
                    core::str::from_utf8(str_bytes).map_err(|_| "invalid utf8 in string prefix")?;

                Ok(AmlValue::String(string))
            }

            // BufferOp
            0x11 => {
                let pkg_start = self.cursor;
                let pkg_len = self.read_pkg_length()?;

                let _buf_size = self.parse_data_object()?;

                let consumed_bytes = self.cursor - pkg_start;
                let data_len = pkg_len
                    .checked_sub(consumed_bytes)
                    .ok_or("BufferOp PkgLength underflowed")?;

                let buf_data = self.take_bytes(data_len)?;
                Ok(AmlValue::Buffer(buf_data))
            }

            // PackageOp
            0x12 => {
                let pkg_start = self.cursor;
                let pkg_len = self.read_pkg_length()?;
                let element_count = self.read_u8()? as usize;

                let consumed_bytes = self.cursor - pkg_start;
                let body_len = pkg_len
                    .checked_sub(consumed_bytes)
                    .ok_or("PackageOp PkgLength underflowed")?;
                let package_bytes = self.take_bytes(body_len)?;
                let mut package_parser = Self::new(package_bytes);
                let mut elements = Vec::with_capacity(element_count);
                for _ in 0..element_count {
                    elements.push(package_parser.parse_data_object()?);
                }
                if !package_parser.is_empty() {
                    return Err("PackageOp contains trailing bytes");
                }
                Ok(AmlValue::Package(elements))
            }
            // VarPackageOp has a computed element count which this parser does not evaluate.
            0x13 => {
                let pkg_start = self.cursor;
                let pkg_len = self.read_pkg_length()?;
                let _element_count = self.parse_data_object()?;

                let consumed_bytes = self.cursor - pkg_start;
                let body_len = pkg_len
                    .checked_sub(consumed_bytes)
                    .ok_or("VarPackageOp PkgLength underflowed")?;
                Ok(AmlValue::Buffer(self.take_bytes(body_len)?))
            }

            // NamePath reference
            0x5C | 0x5E | 0x2E | 0x2F | b'A'..=b'Z' | b'_' => {
                // prefix needs to be eaten by read_name_path
                self.cursor -= 1;
                let name_path = self.read_name_path()?;
                Ok(AmlValue::NamePath(name_path))
            }

            any_other => {
                // extended opcode
                if any_other == 0x5B {
                    let _ext = self.read_u8()?;
                    Ok(AmlValue::Integer(0x5B))
                } else {
                    Ok(AmlValue::Integer(any_other as u64))
                }
            }
        }
    }

    pub fn read_name_path(&mut self) -> Result<AmlNamePath<'a>, &'static str> {
        let start = self.cursor;

        while let Some(ch) = self.peek_u8() {
            if ch == b'\\' || ch == b'^' {
                self.cursor += 1;
            } else {
                break;
            }
        }

        let prefix = self.read_u8()?;
        match prefix {
            0x00 => {} // null
            0x2E => {
                // DualNamePrefix; 2 NameSegs are after
                self.take_bytes(8)?;
            }
            0x2F => {
                // MultiNamePrefix; byte count and then a variable number of NameSegs
                let count = self.read_u8()? as usize;
                self.take_bytes(count * 4)?;
            }
            _ => {
                // one NameSeg (4 bytes, 1st byte was prefix)
                self.take_bytes(3)?;
            }
        }

        let raw = self
            .bytes
            .get(start..self.cursor)
            .ok_or("NamePath slice out of bounds")?;

        Ok(AmlNamePath { raw })
    }

    pub fn read_pkg_length(&mut self) -> Result<usize, &'static str> {
        let lead = self.read_u8()?;
        let byte_count = (lead >> 6) as usize;

        if byte_count == 0 {
            Ok((lead & 0x3F) as usize)
        } else {
            let mut length = (lead & 0x0F) as usize;
            for i in 0..byte_count {
                let next_byte = self.read_u8()?;
                length |= (next_byte as usize) << (4 + i * 8);
            }
            Ok(length)
        }
    }

    #[inline(always)]
    fn read_u8(&mut self) -> Result<u8, &'static str> {
        let b = self
            .bytes
            .get(self.cursor)
            .copied()
            .ok_or("buffer underflow while taking bytes")?;
        self.cursor += 1;
        Ok(b)
    }

    #[inline(always)]
    fn peek_u8(&self) -> Option<u8> {
        self.bytes.get(self.cursor).copied()
    }

    fn read_u16_le(&mut self) -> Result<u16, &'static str> {
        let b = self.take_bytes(2)?;
        Ok(u16::from_le_bytes(b.try_into().unwrap()))
    }

    fn read_u32_le(&mut self) -> Result<u32, &'static str> {
        let b = self.take_bytes(4)?;
        Ok(u32::from_le_bytes(b.try_into().unwrap()))
    }

    fn read_u64_le(&mut self) -> Result<u64, &'static str> {
        let b = self.take_bytes(8)?;
        Ok(u64::from_le_bytes(b.try_into().unwrap()))
    }

    #[inline(always)]
    fn take_bytes(&mut self, len: usize) -> Result<&'a [u8], &'static str> {
        if self.cursor + len <= self.bytes.len() {
            let slice = &self.bytes[self.cursor..self.cursor + len];
            self.cursor += len;
            Ok(slice)
        } else {
            Err("buffer overflow while taking bytes")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AmlParser;
    use crate::ast::AmlValue;

    #[test]
    fn parses_string_packages_used_by_acpi_cids() {
        let bytes = [
            0x12, 0x0b, 0x01, 0x0d, b'P', b'N', b'P', b'0', b'D', b'1', b'0', 0,
        ];
        let mut parser = AmlParser::new(&bytes);

        assert!(matches!(
            parser.parse_data_object(),
            Ok(AmlValue::Package(values))
                if matches!(values.as_slice(), [AmlValue::String("PNP0D10")])
        ));
        assert!(parser.is_empty());
    }

    #[test]
    fn skips_external_terms_without_losing_following_names() {
        let bytes = [
            0x15, b'_', b'S', b'B', b'_', 0x06, 0x08, b'_', b'H', b'I', b'D', 0x0d, b'P', b'N',
            b'P', b'0', b'D', b'1', b'0', 0,
        ];
        let mut parser = AmlParser::new(&bytes);

        assert!(matches!(
            parser.parse_next(),
            Ok(Some(crate::ast::AmlTerm::UnsupportedOpcode(0x15)))
        ));
        assert!(matches!(
            parser.parse_next(),
            Ok(Some(crate::ast::AmlTerm::Name { .. }))
        ));
        assert!(parser.is_empty());
    }
}
