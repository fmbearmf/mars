use log::{Level, LevelFilter, SetLoggerError};

use crate::earlycon_writeln;

pub static LOGGER: Logger = Logger;

pub struct Logger;

const RED_CODE: &str = "\x1b[31m";
const YELLOW_CODE: &str = "\x1b[33m";
const GREEN_CODE: &str = "\x1b[32m";
const BLUE_CODE: &str = "\x1b[34m";
const GRAY_CODE: &str = "\x1b[90m";
const RESET_CODE: &str = "\x1b[0m";

impl Logger {
    pub fn init(&'static self, max_level: LevelFilter) -> Result<(), SetLoggerError> {
        log::set_logger(self).map(|()| log::set_max_level(max_level))
    }
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= Level::Trace
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let level = record.level();

            let color = level_to_color(&level);

            match level {
                Level::Trace | Level::Debug => {
                    let file = record
                        .file()
                        .map_or("<???>", |str| str.rsplit('/').next().unwrap_or(str));
                    let line = record.line().unwrap_or(0);

                    earlycon_writeln!(
                        "{}[{:>5}]{} {}:{}: {}",
                        color,
                        level,
                        RESET_CODE,
                        file,
                        line,
                        record.args()
                    );
                }
                _ => {
                    earlycon_writeln!("{}[{:>5}]{} {}", color, level, RESET_CODE, record.args());
                }
            }
        }
    }

    fn flush(&self) {}
}

fn level_to_color(level: &Level) -> &str {
    match level {
        Level::Error => RED_CODE,
        Level::Warn => YELLOW_CODE,
        Level::Info => GREEN_CODE,
        Level::Debug => BLUE_CODE,
        Level::Trace => GRAY_CODE,
    }
}
