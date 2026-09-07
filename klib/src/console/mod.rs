use core::{
    fmt::{self, Write},
    sync::atomic::{AtomicU64, Ordering},
};

use alloc::boxed::Box;

use crate::{cpu_interface::CpuTopologyId, guard::InterruptGuard, sync::FairSpinlock};

pub trait ConsoleBackend: Write + Send {
    fn flush(&mut self) -> fmt::Result {
        Ok(())
    }
}

pub enum BackendSlot {
    None,
    Fn(fn(fmt::Arguments) -> fmt::Result),
    /// the invariant must (strictly) be that the function is called from a synchronized environment. which is upheld by the spinlock.
    UnsafeFn(unsafe fn(fmt::Arguments) -> fmt::Result),
    Dyn(Box<dyn ConsoleBackend>),
}

pub struct ConsoleManager {
    backend: BackendSlot,
}

impl ConsoleManager {
    pub const fn new() -> Self {
        Self {
            backend: BackendSlot::None,
        }
    }

    pub fn write_fmt(&mut self, args: fmt::Arguments) -> fmt::Result {
        match &mut self.backend {
            BackendSlot::Fn(f) => f(args),
            BackendSlot::UnsafeFn(f) => unsafe { f(args) },
            BackendSlot::Dyn(b) => b.write_fmt(args),
            BackendSlot::None => Ok(()),
        }
    }

    pub fn flush(&mut self) -> fmt::Result {
        match &mut self.backend {
            BackendSlot::Dyn(b) => b.flush(),
            BackendSlot::Fn(_) | BackendSlot::UnsafeFn(_) | BackendSlot::None => Ok(()),
        }
    }
}

static CONSOLE: FairSpinlock<ConsoleManager> = FairSpinlock::new(ConsoleManager::new());
const NO_CPU: u64 = u64::MAX;
static HOLDER: AtomicU64 = AtomicU64::new(NO_CPU);

/// swaps the active backend.
/// returns the previously registered backend.
pub fn set_backend(new_backend: BackendSlot) -> BackendSlot {
    let _guard = InterruptGuard::new(); // prevent deadlocking during replacement
    let cpu = CpuTopologyId::current().to_mpidr();

    if HOLDER.load(Ordering::Relaxed) == cpu {
        panic!("recursive set_backend call on CPU {}", cpu);
    }

    let mut console = CONSOLE.lock();
    HOLDER.store(cpu, Ordering::Relaxed);

    let old = core::mem::replace(&mut console.backend, new_backend);

    HOLDER.store(NO_CPU, Ordering::Release);
    old
}

/// helper for formatting. keeps the lock held across `args` and `\n` to prevent line interleaving.
#[doc(hidden)]
pub fn _print(args: fmt::Arguments, newline: bool) {
    let _guard = InterruptGuard::new(); // prevent deadlocking during printing
    let cpu = CpuTopologyId::current().to_mpidr();

    if HOLDER.load(Ordering::Relaxed) == cpu {
        // gup!!
        return;
    }

    let mut console = CONSOLE.lock();
    HOLDER.store(cpu, Ordering::Relaxed);

    if newline {
        let _ = console.write_fmt(format_args!("{args}\r\n"));
    } else {
        let _ = console.write_fmt(args);
    }

    let _ = console.flush();

    HOLDER.store(NO_CPU, Ordering::Release);
}

/// like `_print` except that it bypasses the lock. mainly for panicking. obviously unsafe (data race) when unsynchronized.
#[doc(hidden)]
pub unsafe fn _evil_print(args: fmt::Arguments, newline: bool) {
    HOLDER.store(NO_CPU, Ordering::Relaxed);

    let mut console = unsafe { CONSOLE.steal() };
    if newline {
        let _ = console.write_fmt(format_args!("{args}\r\n"));
    } else {
        let _ = console.write_fmt(args);
    }

    let _ = console.flush();
}

// this is unsafe if you couldn't tell.
#[macro_export]
macro_rules! unsafe_println_panic_only_unsafe {
    () => {
        $crate::console::_evil_print(core::format_args!(""), true)
    };
    ($($arg:tt)*) => {
        $crate::console::_evil_print(core::format_args!($($arg)*), true)
    }
}

#[macro_export]
macro_rules! print {
    () => {
        $crate::console::_print(core::format_args!(""), false)
    };
    ($($arg:tt)*) => {
        $crate::console::_print(core::format_args!($($arg)*), false)
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::console::_print(core::format_args!(""), true)
    };
    ($($arg:tt)*) => {
        $crate::console::_print(core::format_args!($($arg)*), true)
    }
}
