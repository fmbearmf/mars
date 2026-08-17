use crate::{
    pm::page::mapper::AddressTranslator as AT,
    vm::{dmap_addr_to_phys, phys_addr_to_dmap},
};

#[derive(Debug)]
pub struct KernelAddressTranslator;

impl AT for KernelAddressTranslator {
    fn dmap_to_phys(&self, virt: *mut u8) -> usize {
        dmap_addr_to_phys(virt as _) as _
    }
    fn phys_to_dmap(&self, phys: usize) -> *mut u8 {
        phys_addr_to_dmap(phys as _) as _
    }
}
