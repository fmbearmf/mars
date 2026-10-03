//! Checked firmware topology from Arm IORT. A route does not establish a DMA mapping.

extern crate alloc;

use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Truncated,
    Signature,
    Length,
    Checksum,
    InvalidNode,
    InvalidMapping,
    UnknownReference,
    MissingRoot,
    MissingMapping,
    Ambiguous,
    Unsupported,
    OutOfMemory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Translation {
    None,
    Smmu {
        base: u64,
        span: u64,
        model: u32,
        flags: u32,
        stream_id: u32,
    },
    SmmuV3 {
        base: u64,
        stream_id: u32,
        flags: u32,
        model: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciRoute {
    pub address_bits: u8,
    pub coherent: bool,
    pub translation: Translation,
}

#[derive(Debug)]
pub struct Iort<'a> {
    nodes: Vec<Node<'a>>,
}

#[derive(Debug)]
struct Node<'a> {
    offset: usize,
    bytes: &'a [u8],
    mappings: &'a [u8],
}

fn array<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], Error> {
    let end = offset.checked_add(N).ok_or(Error::Length)?;
    bytes
        .get(offset..end)
        .ok_or(Error::Truncated)?
        .try_into()
        .map_err(|_| Error::Truncated)
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, Error> {
    Ok(u16::from_le_bytes(array(bytes, offset)?))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(array(bytes, offset)?))
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, Error> {
    Ok(u64::from_le_bytes(array(bytes, offset)?))
}

impl<'a> Iort<'a> {
    /// Validate the SDT, node extents, ID mapping arrays and all output references.
    pub fn parse(input: &'a [u8]) -> Result<Self, Error> {
        if input.len() < 48 {
            return Err(Error::Truncated);
        }
        if &input[..4] != b"IORT" {
            return Err(Error::Signature);
        }
        let length = u32_at(input, 4)? as usize;
        if length < 48 {
            return Err(Error::Length);
        }
        let bytes = input.get(..length).ok_or(Error::Length)?;
        if bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) != 0 {
            return Err(Error::Checksum);
        }
        let count = u32_at(bytes, 36)? as usize;
        let mut offset = u32_at(bytes, 40)? as usize;
        if offset < 48 || offset > length || count > (length - offset) / 16 {
            return Err(Error::Length);
        }
        let mut nodes = Vec::new();
        nodes
            .try_reserve_exact(count)
            .map_err(|_| Error::OutOfMemory)?;
        for _ in 0..count {
            let node_length = u16_at(bytes, offset + 1)? as usize;
            if node_length < 16 {
                return Err(Error::InvalidNode);
            }
            let end = offset.checked_add(node_length).ok_or(Error::Length)?;
            let node = bytes.get(offset..end).ok_or(Error::Truncated)?;
            let minimum = minimum_node_length(node)?;
            if node.len() < minimum {
                return Err(Error::InvalidNode);
            }
            // Common node header: mapping_count at +8, mapping_offset at +12.
            let mapping_count = u32_at(node, 8)? as usize;
            let mapping_offset = u32_at(node, 12)? as usize;
            let mappings = if mapping_count == 0 {
                if mapping_offset != 0 {
                    return Err(Error::InvalidMapping);
                }
                &node[0..0]
            } else {
                let mapping_end = mapping_count
                    .checked_mul(20)
                    .and_then(|size| mapping_offset.checked_add(size))
                    .ok_or(Error::InvalidMapping)?;
                if mapping_offset < minimum || node[0] == 0 {
                    return Err(Error::InvalidMapping);
                }
                node.get(mapping_offset..mapping_end)
                    .ok_or(Error::InvalidMapping)?
            };
            nodes.push(Node {
                offset,
                bytes: node,
                mappings,
            });
            offset = end;
        }
        if offset != length {
            return Err(Error::Length);
        }
        let table = Self { nodes };
        for node in &table.nodes {
            for mapping in node.mappings.chunks_exact(20) {
                let flags = u32_at(mapping, 16)?;
                if flags & !1 != 0 {
                    return Err(Error::Unsupported);
                }
                if flags == 0 {
                    let count = u32_at(mapping, 4)?;
                    u32_at(mapping, 0)?
                        .checked_add(count)
                        .ok_or(Error::InvalidMapping)?;
                    u32_at(mapping, 8)?
                        .checked_add(count)
                        .ok_or(Error::InvalidMapping)?;
                }
                table.node_at(u32_at(mapping, 12)? as usize)?;
            }
        }
        Ok(table)
    }

    fn node_at(&self, offset: usize) -> Result<&Node<'a>, Error> {
        let index = self
            .nodes
            .binary_search_by_key(&offset, |node| node.offset)
            .map_err(|_| Error::UnknownReference)?;
        Ok(&self.nodes[index])
    }

    /// Resolve a PCI Requester ID to its first translation unit, or to an ITS.
    /// Missing and ambiguous mappings are errors, never implicit identity mappings.
    pub fn pci_route(&self, segment: u16, requester_id: u16) -> Result<PciRoute, Error> {
        let mut root = None;
        for node in &self.nodes {
            if node.bytes[0] == 2 && u32_at(node.bytes, 28)? == u32::from(segment) {
                if root.replace(node).is_some() {
                    return Err(Error::Ambiguous);
                }
            }
        }
        let root = root.ok_or(Error::MissingRoot)?;
        if root.bytes[3] < 1 {
            return Err(Error::Unsupported);
        }
        let address_bits = root.bytes[32];
        if !(1..=64).contains(&address_bits) {
            return Err(Error::InvalidNode);
        }
        let coherent = match u32_at(root.bytes, 16)? {
            0 => false,
            1 => true,
            _ => return Err(Error::InvalidNode),
        };
        let mut route = PciRoute {
            address_bits,
            coherent,
            translation: Translation::None,
        };
        if root.mappings.is_empty() {
            return Ok(route);
        }
        let mut found = None;
        for mapping in root.mappings.chunks_exact(20) {
            let input = u32_at(mapping, 0)?;
            let count = u32_at(mapping, 4)?;
            let output = u32_at(mapping, 8)?;
            let single = u32_at(mapping, 16)? == 1;
            let rid = u32::from(requester_id);
            // id_count is the maximum offset, so zero still describes one ID.
            let stream_id = if single {
                // IORT permits single mappings on PCI root-complex nodes.
                output
            } else if rid >= input && rid - input <= count {
                output
                    .checked_add(rid - input)
                    .ok_or(Error::InvalidMapping)?
            } else {
                continue;
            };
            if found
                .replace((u32_at(mapping, 12)? as usize, stream_id))
                .is_some()
            {
                return Err(Error::Ambiguous);
            }
        }
        let (reference, stream_id) = found.ok_or(Error::MissingMapping)?;
        let node = self.node_at(reference)?.bytes;
        route.translation = match node[0] {
            0 => Translation::None,
            3 => {
                let base = u64_at(node, 16)?;
                let span = u64_at(node, 24)?;
                if base == 0 || span == 0 || base.checked_add(span).is_none() {
                    return Err(Error::InvalidNode);
                }
                Translation::Smmu {
                    base,
                    span,
                    model: u32_at(node, 32)?,
                    flags: u32_at(node, 36)?,
                    stream_id,
                }
            }
            4 => {
                let base = u64_at(node, 16)?;
                if base == 0 || base & 0xffff != 0 {
                    return Err(Error::InvalidNode);
                }
                Translation::SmmuV3 {
                    base,
                    stream_id,
                    flags: u32_at(node, 24)?,
                    model: u32_at(node, 40)?,
                }
            }
            // Root-to-root cycles, named components and unknown node types are not PCI DMA routes.
            _ => return Err(Error::Unsupported),
        };
        Ok(route)
    }
}

fn minimum_node_length(node: &[u8]) -> Result<usize, Error> {
    Ok(match node[0] {
        0 => 20usize
            .checked_add(
                (u32_at(node, 16)? as usize)
                    .checked_mul(4)
                    .ok_or(Error::Length)?,
            )
            .ok_or(Error::Length)?,
        2 => {
            if node[3] == 0 {
                32
            } else {
                36
            }
        }
        3 => 60,
        4 => {
            if node[3] == 0 {
                60
            } else {
                68
            }
        }
        _ => 16,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn put32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn checksum(bytes: &mut [u8]) {
        bytes[9] = 0;
        bytes[9] = 0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
    }

    fn node(kind: u8, length: usize) -> Vec<u8> {
        let mut node = vec![0; length];
        node[0] = kind;
        node[1..3].copy_from_slice(&(length as u16).to_le_bytes());
        node[3] = 1;
        node
    }

    fn root(mappings: usize) -> Vec<u8> {
        let mut root = node(2, 36 + mappings * 20);
        put32(&mut root, 16, 1);
        put32(&mut root, 28, 7);
        root[32] = 48;
        put32(&mut root, 8, mappings as u32);
        if mappings != 0 {
            put32(&mut root, 12, 36);
        }
        root
    }

    fn mapping(
        bytes: &mut [u8],
        index: usize,
        input: u32,
        count: u32,
        output: u32,
        reference: u32,
    ) {
        let at = 36 + index * 20;
        for (i, value) in [input, count, output, reference, 0].into_iter().enumerate() {
            put32(bytes, at + i * 4, value);
        }
    }

    fn table(nodes: &[Vec<u8>]) -> Vec<u8> {
        let length = 48 + nodes.iter().map(Vec::len).sum::<usize>();
        let mut bytes = vec![0; 48];
        bytes[..4].copy_from_slice(b"IORT");
        put32(&mut bytes, 4, length as u32);
        bytes[8] = 5;
        put32(&mut bytes, 36, nodes.len() as u32);
        put32(&mut bytes, 40, 48);
        for node in nodes {
            bytes.extend_from_slice(node);
        }
        checksum(&mut bytes);
        bytes
    }

    fn direct() -> Vec<u8> {
        let mut root = root(1);
        let target = 48 + root.len() as u32;
        mapping(&mut root, 0, 0x100, 0xff, 0x200, target);
        let mut its = node(0, 24);
        put32(&mut its, 16, 1);
        put32(&mut its, 20, 9);
        table(&[root, its])
    }

    #[test]
    fn direct_route_uses_real_common_header_offsets() {
        let bytes = direct();
        let iort = Iort::parse(&bytes).unwrap();
        assert_eq!(
            iort.pci_route(7, 0x100),
            Ok(PciRoute {
                address_bits: 48,
                coherent: true,
                translation: Translation::None
            })
        );
        assert!(iort.pci_route(7, 0x1ff).is_ok());
        assert_eq!(iort.pci_route(7, 0x200), Err(Error::MissingMapping));
        assert_eq!(iort.pci_route(8, 0x100), Err(Error::MissingRoot));
    }

    #[test]
    fn no_mappings_is_reported_as_topology_not_dma_authorization() {
        let bytes = table(&[root(0)]);
        assert_eq!(
            Iort::parse(&bytes)
                .unwrap()
                .pci_route(7, 0)
                .unwrap()
                .translation,
            Translation::None
        );
    }

    #[test]
    fn smmuv3_stream_id_and_model_offsets() {
        let mut root = root(1);
        let target = 48 + root.len() as u32;
        mapping(&mut root, 0, 0x100, 1, 0x123, target);
        let mut smmu = node(4, 68);
        put64(&mut smmu, 16, 0x1_0000_0000);
        put32(&mut smmu, 24, 0x10);
        put32(&mut smmu, 40, 2);
        put32(&mut smmu, 44, 88);
        let bytes = table(&[root, smmu]);
        assert_eq!(
            Iort::parse(&bytes)
                .unwrap()
                .pci_route(7, 0x101)
                .unwrap()
                .translation,
            Translation::SmmuV3 {
                base: 0x1_0000_0000,
                stream_id: 0x124,
                flags: 0x10,
                model: 2
            }
        );
    }

    #[test]
    fn smmuv2_route_with_single_requester() {
        let mut root = root(1);
        let target = 48 + root.len() as u32;
        mapping(&mut root, 0, 0x100, 0, 0x345, target);
        let mut smmu = node(3, 60);
        put64(&mut smmu, 16, 0x100000);
        put64(&mut smmu, 24, 0x20000);
        put32(&mut smmu, 32, 1);
        put32(&mut smmu, 36, 2);
        let bytes = table(&[root, smmu]);
        let iort = Iort::parse(&bytes).unwrap();
        assert_eq!(
            iort.pci_route(7, 0x100).unwrap().translation,
            Translation::Smmu {
                base: 0x100000,
                span: 0x20000,
                model: 1,
                flags: 2,
                stream_id: 0x345
            }
        );
        assert_eq!(iort.pci_route(7, 0x101), Err(Error::MissingMapping));
    }

    #[test]
    fn single_mapping_ignores_input_range() {
        let mut bytes = direct();
        put32(&mut bytes, 48 + 36, u32::MAX);
        put32(&mut bytes, 48 + 36 + 4, u32::MAX);
        put32(&mut bytes, 48 + 36 + 16, 1);
        checksum(&mut bytes);
        let iort = Iort::parse(&bytes).unwrap();
        assert!(iort.pci_route(7, 0).is_ok());
        assert!(iort.pci_route(7, u16::MAX).is_ok());
    }

    #[test]
    fn truncation_and_table_checksums() {
        let bytes = direct();
        for length in 0..bytes.len() {
            assert!(Iort::parse(&bytes[..length]).is_err());
        }
        let mut corrupt = bytes.clone();
        corrupt[10] ^= 1;
        assert!(matches!(Iort::parse(&corrupt), Err(Error::Checksum)));
        corrupt[..4].copy_from_slice(b"MCFG");
        assert!(matches!(Iort::parse(&corrupt), Err(Error::Signature)));
        for length in 0..48 {
            let mut corrupt = bytes.clone();
            put32(&mut corrupt, 4, length);
            assert!(Iort::parse(&corrupt).is_err());
        }
    }

    #[test]
    fn node_and_mapping_bounds() {
        for value in [0, 1, 15, u16::MAX] {
            let mut bytes = direct();
            bytes[49..51].copy_from_slice(&value.to_le_bytes());
            checksum(&mut bytes);
            assert!(Iort::parse(&bytes).is_err());
        }
        for offset in [0, 8, 16, 32, 35, u32::MAX] {
            let mut bytes = direct();
            put32(&mut bytes, 48 + 12, offset);
            checksum(&mut bytes);
            assert!(Iort::parse(&bytes).is_err());
        }
        let mut bytes = direct();
        put32(&mut bytes, 48 + 8, u32::MAX);
        checksum(&mut bytes);
        assert!(Iort::parse(&bytes).is_err());
    }

    #[test]
    fn mapping_arithmetic_does_not_wrap() {
        for offset in [0, 8] {
            let mut bytes = direct();
            put32(&mut bytes, 48 + 36 + offset, u32::MAX);
            checksum(&mut bytes);
            assert!(matches!(Iort::parse(&bytes), Err(Error::InvalidMapping)));
        }
    }

    #[test]
    fn references_must_be_exact_node_starts() {
        for reference in [0, 16, 49, 105, u32::MAX] {
            let mut bytes = direct();
            put32(&mut bytes, 48 + 36 + 12, reference);
            checksum(&mut bytes);
            assert!(matches!(Iort::parse(&bytes), Err(Error::UnknownReference)));
        }
    }

    #[test]
    fn duplicate_roots_and_overlapping_mappings_are_not_guessed() {
        let bytes = table(&[root(0), root(0)]);
        assert_eq!(
            Iort::parse(&bytes).unwrap().pci_route(7, 0),
            Err(Error::Ambiguous)
        );
        let mut root = root(2);
        let target = 48 + root.len() as u32;
        mapping(&mut root, 0, 0x100, 0x100, 0, target);
        mapping(&mut root, 1, 0x200, 0xff, 0, target);
        let bytes = table(&[root, node(0, 20)]);
        assert_eq!(
            Iort::parse(&bytes).unwrap().pci_route(7, 0x200),
            Err(Error::Ambiguous)
        );
    }

    #[test]
    fn cycles_and_unsupported_routes_are_errors() {
        let mut bytes = direct();
        put32(&mut bytes, 48 + 36 + 12, 48);
        checksum(&mut bytes);
        assert_eq!(
            Iort::parse(&bytes).unwrap().pci_route(7, 0x100),
            Err(Error::Unsupported)
        );
        bytes[104] = 99;
        put32(&mut bytes, 48 + 36 + 12, 104);
        checksum(&mut bytes);
        assert_eq!(
            Iort::parse(&bytes).unwrap().pci_route(7, 0x100),
            Err(Error::Unsupported)
        );
    }

    #[test]
    fn byte_mutations_never_panic_during_validation_or_routing() {
        let original = direct();
        for offset in 0..original.len() {
            for value in 0..=u8::MAX {
                let mut bytes = original.clone();
                bytes[offset] = value;
                checksum(&mut bytes);
                if let Ok(iort) = Iort::parse(&bytes) {
                    for (segment, rid) in [(0, 0), (7, 0x100), (7, u16::MAX)] {
                        let _ = iort.pci_route(segment, rid);
                    }
                }
            }
        }
    }

    #[test]
    fn unknown_addressability_and_coherency_are_errors() {
        for limit in [0, 65, 255] {
            let mut bytes = direct();
            bytes[48 + 32] = limit;
            checksum(&mut bytes);
            assert_eq!(
                Iort::parse(&bytes).unwrap().pci_route(7, 0x100),
                Err(Error::InvalidNode)
            );
        }
        let mut bytes = direct();
        put32(&mut bytes, 48 + 16, 2);
        checksum(&mut bytes);
        assert_eq!(
            Iort::parse(&bytes).unwrap().pci_route(7, 0x100),
            Err(Error::InvalidNode)
        );
        bytes[48 + 3] = 0;
        checksum(&mut bytes);
        assert_eq!(
            Iort::parse(&bytes).unwrap().pci_route(7, 0x100),
            Err(Error::Unsupported)
        );
    }
}
