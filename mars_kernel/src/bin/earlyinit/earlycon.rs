use arm_pl011_uart::{LineConfig, PL011Registers, Uart, UniqueMmioPointer};
use core::{fmt::Write, ptr::NonNull};
use mars_kernel::vm::mmio_addr_to_iomap;
use spin::Mutex;

const UART_ADDRESS: *mut PL011Registers = (mmio_addr_to_iomap(0x0900_0000)) as *mut PL011Registers;

pub static EARLYCON: Mutex<Option<EarlyCon>> = Mutex::new(None);

#[macro_export]
macro_rules! earlycon_write {
    ($($arg:tt)*) => {{
        if let Some(uart) = crate::earlyinit::earlycon::EARLYCON.lock().as_mut() {
            let _ = core::write!(uart.uart, $($arg)*);
        }
    }};
}

#[macro_export]
macro_rules! earlycon_writeln {
    ($($arg:tt)*) => {{
        if let Some(uart) = crate::earlyinit::earlycon::EARLYCON.lock().as_mut() {
            let _ = core::writeln!(uart.uart, $($arg)*);
        }
    }};
}

pub struct EarlyCon<'a> {
    pub uart: Uart<'a>,
}

impl<'a> EarlyCon<'a> {
    pub fn new() -> Self {
        let uart_ptr = unsafe { UniqueMmioPointer::new(NonNull::new(UART_ADDRESS).unwrap()) };
        let mut uart = Uart::new(uart_ptr);

        let line_conf = LineConfig {
            data_bits: arm_pl011_uart::DataBits::Bits8,
            parity: arm_pl011_uart::Parity::None,
            stop_bits: arm_pl011_uart::StopBits::One,
        };
        _ = uart.enable(line_conf, 115_200, 16_000_000);
        _ = writeln!(uart, "UART {:p} enabled", UART_ADDRESS);

        Self { uart }
    }
}
