#![no_std]

#[cfg(test)]
mod tests;

#[cfg(test)]
use line::LINE_CAPACITY;

mod command;
mod line;
mod monitor;
mod render;
mod shell;

pub use command::{CommandKind, ParsedLine, parse};
pub use line::{Complete, Editing, Line, Parsed};
pub use monitor::{
    CommandRequest, CpuState, Snapshot, Telemetry, ThreadKind, ThreadMetadata, ThreadState,
};
pub use render::OUTPUT_CAPACITY;
pub use shell::{PROMPT, Shell};
