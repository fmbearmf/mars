use alloc::boxed::Box;
use core::sync::atomic::Ordering;
use hal::interrupt::InterruptGuard;
use klib::{
    allocator_support::KernelAddressTranslator,
    cache::clean_dcache_range,
    cpu_interface::{CpuIdLogical, CpuTopologyId},
    hardware::device::DeviceNode,
    per_cpu::PerCpu,
    pm::page::mapper::AddressTranslator,
    scheduler::GLOBAL_SCHEDULER,
    smccc::{PsciError, cpu_on},
    stack::Stack,
};

use crate::{
    DEVICE_TREE,
    earlyinit::{
        idle::idle_init,
        platform::{DISABLE_IRQ, ENABLE_IRQ, filter_fundamental, filter_others},
    },
    lut::{DEVICE_TABLE, DeviceCallback},
};

pub unsafe fn boot_secondary(
    core: CpuTopologyId,
    logical_id: CpuIdLogical,
    stack: Stack,
    addr_translator: impl Fn(usize) -> usize,
) -> Result<(), PsciError> {
    use log::*;

    let stack_top = stack.as_ptr_range().end as usize;

    let args = Box::new(unsafe {
        hal::secondary::prepare(stack_top, secondary_init, logical_id.to_u32())
    });

    let args_ptr = args.as_ref() as *const _ as *const u8;
    unsafe {
        clean_dcache_range(
            args_ptr as *const _ as _,
            core::mem::size_of_val(args.as_ref()),
        )
    };

    let trampoline_phys = addr_translator(hal::secondary::entry_address()) as u64;
    let args_phys = KernelAddressTranslator.dmap_to_phys(args_ptr as _) as u64;

    info!(
        "Waking up CPU {} at {:#x}",
        core.to_logical()
            .map(|c| c.to_u32())
            .map_or(-1, |c| c as i32),
        args_phys
    );

    cpu_on(core, trampoline_phys, args_phys)?;

    let pcpu = PerCpu::get(logical_id.to_usize()).expect("invalid logical_id passed");

    while pcpu.ready.load(Ordering::Acquire) != true {}
    // idle_entry publishes readiness after switching away from the temporary stack
    drop(stack);

    trace!(
        "SMP: CPU {} is confirmed to be ready. moving on...",
        logical_id.to_u32()
    );

    Ok(())
}

#[allow(dead_code, reason = "called indirectly")]
pub unsafe extern "C" fn secondary_init(cpu_id: u32) -> ! {
    let cpu_id = CpuIdLogical::new(cpu_id);
    PerCpu::register_local(cpu_id.to_usize()).expect("invalid cpu_id passed to secondary core!");
    InterruptGuard::disable();

    GLOBAL_SCHEDULER.register_cpu(cpu_id);

    secondary_main();

    idle_init()
}

fn secondary_main() {
    let dt = DEVICE_TREE.borrow();

    fn init_devices<'a>(i: impl Iterator<Item = &'a DeviceNode>) {
        for (func, node) in i.filter_map(|node| {
            node.compatible
                .iter()
                .find_map(|tag| DEVICE_TABLE.get(tag))
                .filter(|func| matches!(func, DeviceCallback::EveryCore(_)))
                .map(|func| (func, node))
        }) {
            let f = match func {
                DeviceCallback::EveryCore((_, f)) => f,
                _ => unimplemented!(),
            };
            f(node, ENABLE_IRQ, DISABLE_IRQ);
        }
    }

    init_devices(dt.nodes.iter().filter(filter_fundamental));
    init_devices(dt.nodes.iter().filter(filter_others));
}
