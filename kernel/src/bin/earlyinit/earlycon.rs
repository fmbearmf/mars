use arm_pl011_uart::{LineConfig, PL011Registers, Uart, UniqueMmioPointer};
use core::{fmt::Write, ptr::NonNull};
use spin::Mutex;

pub static EARLYCON: Mutex<Option<EarlyCon>> = Mutex::new(None);

#[macro_export]
macro_rules! earlycon_write {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        if let Some(uart) = crate::earlyinit::earlycon::EARLYCON.lock().as_mut() {
            let _ = core::write!(uart.uart, $($arg)*);
        }
    }};
}

#[macro_export]
macro_rules! earlycon_writeln {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        if let Some(uart) = crate::earlyinit::earlycon::EARLYCON.lock().as_mut() {
            let _ = core::writeln!(uart.uart, $($arg)*);
        }
    }};
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
}
