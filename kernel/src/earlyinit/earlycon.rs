use arm_pl011_uart::{LineConfig, PL011Registers, Uart, UniqueMmioPointer};
use core::{
    cell::SyncUnsafeCell,
    fmt::{self, Write},
    ptr::NonNull,
};

pub static EARLYCON: SyncUnsafeCell<Option<EarlyCon>> = SyncUnsafeCell::new(None);

/// SAFETY: call from console subsystem only. unsynchronized.
pub unsafe fn earlycon_write_impl(f: fmt::Arguments) -> fmt::Result {
    // UB when panicking (codegen believes this has exclusive access, but panic calls it without synchronization).
    // arm_pl011_uart forces my hand, because you can't write without `&mut self`.
    // TODO: fix that probably.
    if let Some(earlycon) = unsafe { (*EARLYCON.get()).as_mut() } {
        earlycon.uart.write_fmt(f)
    } else {
        Ok(())
    }
}

pub struct EarlyCon<'a> {
    pub uart: Uart<'a>,
}

impl<'a> EarlyCon<'a> {
    pub fn new(serial_uart_addr: usize) -> Self {
        let uart_ptr = unsafe {
            UniqueMmioPointer::new(NonNull::new(serial_uart_addr as *mut PL011Registers).unwrap())
        };
        let mut uart = Uart::new(uart_ptr);

        let line_conf = LineConfig {
            data_bits: arm_pl011_uart::DataBits::Bits8,
            parity: arm_pl011_uart::Parity::None,
            stop_bits: arm_pl011_uart::StopBits::One,
        };
        _ = uart.enable(line_conf, 115_200, 16_000_000);
        _ = writeln!(uart, "UART {:#x} enabled", serial_uart_addr);

        Self { uart }
    }

    pub fn switch(&mut self, serial_uart_addr: usize) {
        let uart_ptr = unsafe {
            UniqueMmioPointer::new(NonNull::new(serial_uart_addr as *mut PL011Registers).unwrap())
        };
        self.uart = Uart::new(uart_ptr);
    }
}
