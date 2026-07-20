use core::{borrow::Borrow, mem::MaybeUninit};

use klib::{
    cpu_interface::{CpuTopologyId, init_cpu_maps},
    hardware::{
        device::{DeviceClass, DeviceInitPriority, DeviceNode, IrqFn},
        irq::CallbackError,
        resource::Resource,
    },
    stack::Stack,
    this_cpu,
    vm::{PAGE_SIZE, user::PAGE_DESCRIPTORS},
};
use protocol::BootInfo;
use uefi::mem::memory_map::{MemoryMap, MemoryMapMut};

use crate::{
    __KBASE, DEVICE_TREE, GLOBAL_SCHEDULER, KALLOCATOR, KERNEL_ADDRESS_SPACE,
    earlyinit::{
        acpi::acpi_init,
        earlycon::{EARLYCON, EarlyCon},
        mem::{
            clone_and_process_mmap, create_page_descriptors, populate_alloc_stage0,
            populate_alloc_stage1, switch_to_new_page_tables,
        },
        mmu::init_mmu,
        smp::boot_secondary,
    },
    interrupt::get_interrupt_controller,
    log::LOGGER,
    lut::{DEVICE_TABLE, DeviceCallback},
};

mod sealed {
    use atomic_enum::atomic_enum;
    use core::{marker::PhantomData, ptr, sync::atomic::Ordering};

    use super::{BootInfo, MaybeUninit};

    #[atomic_enum]
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    enum State {
        Fresh = 0,
        Uninit,
        Init,
    }

    // storage for boot info struct
    static mut BOOT_INFO: MaybeUninit<BootInfo> = MaybeUninit::uninit();
    static STATE: AtomicState = AtomicState::new(State::Fresh);

    pub struct BootInfoInitToken {
        _private: (),
    }

    impl !Send for BootInfoInitToken {}
    impl !Sync for BootInfoInitToken {}
    impl !Clone for BootInfoInitToken {}
    impl !Copy for BootInfoInitToken {}

    pub struct BootInfoToken {
        _private: (),
        _phantom: PhantomData<BootInfo>,
    }

    impl !Send for BootInfoToken {}
    impl !Sync for BootInfoToken {}
    impl !Clone for BootInfoToken {}
    impl !Copy for BootInfoToken {}

    impl BootInfoInitToken {
        /// Create a BootInfoToken.
        /// `None` if new() has already been called.
        pub fn new() -> Option<Self> {
            if STATE
                .compare_exchange(
                    State::Fresh,
                    State::Uninit,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                Some(Self { _private: () })
            } else {
                None
            }
        }

        /// returns `None` if already initialized.
        /// returns `Some(BootInfoToken)` when successful.
        /// safety: pointer is initialized and safe to copy from (and obviously valid).
        pub unsafe fn init(self, pointer: *mut BootInfo) -> Option<BootInfoToken> {
            if STATE.load(Ordering::Acquire) == State::Uninit {
                #[allow(
                    static_mut_refs,
                    reason = "BootInfoToken wraps the static mutable (which is inaccessible outside)"
                )]
                unsafe {
                    ptr::copy_nonoverlapping(pointer, BOOT_INFO.as_mut_ptr(), 1)
                };

                STATE.store(State::Init, Ordering::Release);
            } else {
                return None;
            }

            Some(BootInfoToken {
                _private: (),
                _phantom: PhantomData,
            })
        }
    }

    impl BootInfoToken {
        pub fn get<'a>(&'a self) -> &'a BootInfo {
            debug_assert_eq!(STATE.load(Ordering::Acquire), State::Init);

            #[allow(
                static_mut_refs,
                reason = "BootInfoToken wraps the static mutable (which is inaccessible outside)"
            )]
            unsafe {
                BOOT_INFO.assume_init_ref()
            }
        }

        pub fn get_mut<'a>(&'a mut self) -> &'a mut BootInfo {
            debug_assert_eq!(STATE.load(Ordering::Acquire), State::Init);

            #[allow(
                static_mut_refs,
                reason = "BootInfoToken wraps the static mutable (which is inaccessible outside)"
            )]
            unsafe {
                BOOT_INFO.assume_init_mut()
            }
        }
    }
}

pub use sealed::*;

pub fn uefi_arm64_bootstrap(mut boot_info_token: BootInfoToken) {
    use log::*;

    let boot_info = boot_info_token.get_mut();
    let load_addr = boot_info.kernel_load_physical_address;

    {
        let mut lock = EARLYCON.lock();
        *lock = Some(EarlyCon::new(boot_info.serial_uart_address));
    }

    LOGGER
        .init(LevelFilter::Trace)
        .expect("failed to init logger");

    info!(
        "Mars {}, provided under the {} license.",
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_LICENSE")
    );

    trace!("address of bootinfo: {:#p}", &boot_info);

    trace!("init_mmu addr: {:#p}", init_mmu as *const ());
    init_mmu(boot_info.page_table_root);

    let uefi_mmap = &mut boot_info.memory_map;
    uefi_mmap.sort();

    //trace!(
    //    "uefi_mmap @ {:p}",
    //    uefi_mmap.buffer() as *const _ as *const ()
    //);

    let uefi_mmap = clone_and_process_mmap(uefi_mmap);
    trace!("processed uefi_mmap @ {:p}", uefi_mmap.buffer() as *const _);

    //for desc in uefi_mmap.entries() {
    //    trace!("{:x?}", desc);
    //}

    populate_alloc_stage0(&uefi_mmap);

    let new_pt = unsafe { switch_to_new_page_tables(|| uefi_mmap.entries(), &KALLOCATOR) };

    unsafe { KALLOCATOR.transition_dmap() };

    let (page_descriptors, range) = create_page_descriptors();
    PAGE_DESCRIPTORS.init(page_descriptors, range);

    KERNEL_ADDRESS_SPACE.init_from_table(new_pt);

    populate_alloc_stage1(&uefi_mmap);

    acpi_init(&boot_info_token);

    {
        let dt = DEVICE_TREE.borrow();

        fn init_devices<'a>(i: impl Iterator<Item = &'a DeviceNode>) {
            for (func, node) in i.filter_map(|node| {
                node.compatible
                    .iter()
                    .find_map(|tag| DEVICE_TABLE.get(tag))
                    .map(|func| (func, node))
            }) {
                let f = match func {
                    DeviceCallback::Once(f) => f,
                    DeviceCallback::EveryCore((f, _)) => f,
                };
                f(node, ENABLE_IRQ, DISABLE_IRQ);
            }
        }

        init_devices(dt.nodes.iter().filter(filter_fundamental));
        init_devices(dt.nodes.iter().filter(filter_others));

        let create_cpu_iter = || {
            dt.nodes
                .iter()
                .filter(filter_fundamental)
                .filter_map(|node| match &node.class {
                    DeviceClass::Cpu { id, .. } => Some(id.clone()),
                    _ => None,
                })
        };

        init_cpu_maps(create_cpu_iter());

        info!("Waking up secondary CPUs.");
        for cpu in create_cpu_iter() {
            if cpu == CpuTopologyId::current() {
                continue;
            }

            let logical = cpu.to_logical().expect("invalid topology id");

            let stack = Stack::new(PAGE_SIZE, 16).expect("unable to allocate stack");

            unsafe {
                boot_secondary(cpu, logical, stack, |addr| {
                    (addr - &__KBASE as *const _ as usize) + load_addr
                })
                .expect("error booting secondary core")
            };
        }
    }

    GLOBAL_SCHEDULER.register_cpu(this_cpu!().id);
}

pub fn filter_fundamental<T>(n: &T) -> bool
where
    T: Borrow<DeviceNode>,
{
    n.borrow().priority >= DeviceInitPriority::Fundamental
}

pub fn filter_others<T>(n: &T) -> bool
where
    T: Borrow<DeviceNode>,
{
    n.borrow().priority != DeviceInitPriority::Fundamental
}

pub const ENABLE_IRQ: IrqFn = |resource| {
    use log::*;

    let ic = get_interrupt_controller();

    let result = match resource {
        Resource::Irq(irq) => {
            ic.enable_interrupt(*irq)
                .map_err(|_| CallbackError::FailedToEnable)?;
            trace!("device driver enabled IRQ {}", irq);

            Ok(())
        }
        _ => {
            error!("device driver tried to enable an IRQ with an invalid Resource.");
            Err(CallbackError::FailedToEnable)
        }
    };

    result
};

pub const DISABLE_IRQ: IrqFn = |resource| {
    use log::*;

    let ic = get_interrupt_controller();

    let result = match resource {
        Resource::Irq(irq) => {
            ic.disable_interrupt(*irq)
                .map_err(|_| CallbackError::FailedToDisable)?;
            trace!("device driver disabled IRQ {}", irq);

            Ok(())
        }
        _ => {
            error!("device driver tried to disable an IRQ with an invalid Resource.");
            Err(CallbackError::FailedToDisable)
        }
    };

    result
};
