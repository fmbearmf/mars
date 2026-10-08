use crate::line::{Complete, Line, Parsed};

/// the result of parsing a completed command line
pub struct ParsedLine {
    pub(crate) line: Line<Parsed>,
    pub(crate) command: CommandKind,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CommandKind {
    Help,
    Echo(usize),
    Uptime,
    Cpu,
    Memory,
    Sched,
    Cpus,
    Empty,
    Unknown,
}

/// parse a line only after its editing state has been consumed
///
/// ```compile_fail
/// use kernel_shell::{parse, Line, Editing};
/// let line = Line::<Editing>::new();
/// let _ = parse(line);
/// ```
///
/// ```compile_fail
/// use kernel_shell::{Line, Editing};
/// let line = Line::<Editing>::new();
/// let _ = line.complete().push(b'x');
/// ```
pub fn parse(line: Line<Complete>) -> ParsedLine {
    let command = if line.as_bytes() == b"help" {
        CommandKind::Help
    } else if let Some(text) = line.as_bytes().strip_prefix(b"echo ") {
        CommandKind::Echo(line.len - text.len())
    } else if line.as_bytes() == b"uptime" {
        CommandKind::Uptime
    } else if line.as_bytes() == b"cpu" {
        CommandKind::Cpu
    } else if line.as_bytes() == b"memory" {
        CommandKind::Memory
    } else if line.as_bytes() == b"sched" {
        CommandKind::Sched
    } else if line.as_bytes() == b"cpus" {
        CommandKind::Cpus
    } else if line.len == 0 {
        CommandKind::Empty
    } else {
        CommandKind::Unknown
    };

    ParsedLine {
        line: Line {
            bytes: line.bytes,
            len: line.len,
            state: core::marker::PhantomData,
        },
        command,
    }
}
