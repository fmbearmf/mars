#![no_std]
#![feature(const_option_ops)]
#![feature(const_trait_impl)]
#![feature(generic_atomic)]
#![feature(new_range_api)]

extern crate alloc;

use core::{
    fmt::{self, Write},
    str::from_utf8,
};

pub mod context;
pub mod cpu_interface;
pub mod exception;
pub mod hardware;
pub mod hash_map;
pub mod interrupt;
pub mod pm;
pub mod process;
pub mod scheduler;
pub mod smccc;
pub mod sync;
pub mod thread;
pub mod timer;
pub mod vcpu;
pub mod vm;

pub fn bytes_to_human_readable(mut bytes: u64, buf: &mut [u8]) -> &str {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    const K: u64 = 1024;

    let mut unit_i = 0;
    while bytes >= K && unit_i < UNITS.len() - 1 {
        bytes /= K;
        unit_i += 1;
    }

    struct BufWriter<'a> {
        buf: &'a mut [u8],
        pos: usize,
    }

    impl<'a> Write for BufWriter<'a> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            let bytes = s.as_bytes();
            if self.pos + bytes.len() > self.buf.len() {
                return Err(fmt::Error);
            }

            self.buf[self.pos..self.pos + bytes.len()].copy_from_slice(bytes);
            self.pos += bytes.len();
            Ok(())
        }
    }

    let mut writer = BufWriter { buf, pos: 0 };
    _ = write!(writer, "{} {}", bytes, UNITS[unit_i]);
    from_utf8(&writer.buf[..writer.pos]).unwrap()
}
