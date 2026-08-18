pub mod extended;
pub mod standard;

use crate::{
    address::Bdf,
    capability::{
        extended::{ExtendedCapIter, ExtendedCapabilityId},
        standard::{StandardCapIter, StandardCapabilityId},
    },
    ecam::Ecam,
};

pub fn find_capability(ecam: &Ecam, bdf: Bdf, target_id: StandardCapabilityId) -> Option<u8> {
    StandardCapIter::new(ecam, bdf).find_map(
        |(id, offset)| {
            if id == target_id { Some(offset) } else { None }
        },
    )
}

/// find PCIe extended cap by ID
pub fn find_extended_capability(
    ecam: &Ecam,
    bdf: Bdf,
    target_id: ExtendedCapabilityId,
) -> Option<u16> {
    ExtendedCapIter::new(ecam, bdf).find_map(
        |(id, offset)| {
            if id == target_id { Some(offset) } else { None }
        },
    )
}
