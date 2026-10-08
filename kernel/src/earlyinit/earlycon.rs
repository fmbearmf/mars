use arm_pl011_uart::{LineConfig, PL011Registers, Uart, UniqueMmioPointer};
use core::{
    fmt::{self, Write},
    ptr::NonNull,
};
use klib::sync::FairSpinlock;

pub static EARLYCON: FairSpinlock<Option<EarlyCon>> = FairSpinlock::new(None);

pub fn earlycon_write_impl(f: fmt::Arguments) -> fmt::Result {
    let mut earlycon = EARLYCON.lock();
    if let Some(earlycon) = earlycon.as_mut() {
        earlycon.uart.write_fmt(f)
    } else {
        Ok(())
    }
}

pub fn earlycon_read_byte() -> Result<Option<u8>, ()> {
    let mut earlycon = EARLYCON.lock();
    match earlycon.as_mut() {
        Some(earlycon) => earlycon.uart.read_word().map_err(|_| ()),
        None => Ok(None),
    }
}

pub fn earlycon_panic_write(args: fmt::Arguments<'_>) {
    let Some(mut earlycon) = EARLYCON.try_lock() else {
        return;
    };
    let Some(earlycon) = earlycon.as_mut() else {
        return;
    };
    let mut writer = PanicUartWriter(&mut earlycon.uart);
    let _ = writer.write_fmt(args);
}

struct PanicUartWriter<'a, 'uart>(&'a mut Uart<'uart>);

impl Write for PanicUartWriter<'_, '_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        const TX_POLL_LIMIT: usize = 10_000;

        for byte in text.bytes() {
            let mut ready = false;
            for _ in 0..TX_POLL_LIMIT {
                if !self.0.is_tx_fifo_full() {
                    ready = true;
                    break;
                }
                core::hint::spin_loop();
            }
            if !ready {
                return Ok(());
            }
            self.0.write_word(byte);
        }
        Ok(())
    }
}

pub struct EarlyCon<'a> {
    pub uart: Uart<'a>,
}

// safety: moving the uniquely owned UART between threads does not create aliases
unsafe impl Send for EarlyCon<'_> {}

impl<'a> EarlyCon<'a> {
    pub fn new(serial_uart_addr: usize) -> Self {
        // safety: the platform supplies an aligned direct-map address for a valid PL011 register block whose mapping outlives this UART, and this UART is its sole owner
        let uart_ptr = unsafe {
            UniqueMmioPointer::new(NonNull::new(serial_uart_addr as *mut PL011Registers).unwrap())
        };
        let mut uart = Uart::new(uart_ptr);

        let line_conf = LineConfig {
            data_bits: arm_pl011_uart::DataBits::Bits8,
            parity: arm_pl011_uart::Parity::None,
            stop_bits: arm_pl011_uart::StopBits::One,
        };
        _ = uart.enable(line_conf, 115_200, 100_000_000);
        _ = writeln!(uart, "UART {:#x} enabled", serial_uart_addr);

        Self { uart }
    }
}
