use crate::command::{CommandKind, ParsedLine};
use crate::monitor::{CommandRequest, CpuState, Snapshot, Telemetry};

pub const OUTPUT_CAPACITY: usize = 256;

pub(crate) fn render_command_stream<T: Telemetry>(
    parsed: ParsedLine,
    telemetry: &mut T,
    emit: &mut impl FnMut(&[u8]),
) {
    if parsed.command == CommandKind::Work {
        emit(b"work: starting bounded CPU workload (CPU numbers are kernel logical IDs)\r\n");
        let _ = telemetry.snapshot(CommandRequest::Work, 0);
        return;
    }

    if parsed.command != CommandKind::Cpus {
        let mut buffer = [0; OUTPUT_CAPACITY];
        let mut output = Rendered::new(&mut buffer);

        render_command(parsed, &mut output, telemetry);

        let len = output.len;
        drop(output);
        emit(&buffer[..len]);

        return;
    }

    for index in 0.. {
        let Some(snapshot) = telemetry.snapshot(CommandRequest::Cpus, index) else {
            break;
        };

        let mut buffer = [0; OUTPUT_CAPACITY];
        let mut output = Rendered::new(&mut buffer);

        render_snapshot(Some(snapshot), &mut output);

        let len = output.len;
        drop(output);
        emit(&buffer[..len]);
    }
}

pub(crate) fn render_command<T: Telemetry>(
    parsed: ParsedLine,
    output: &mut Rendered<'_>,
    telemetry: &mut T,
) {
    match parsed.command {
        CommandKind::Help => {
            output.bytes(b"commands: help, echo <text>, uptime, cpu, memory, sched, cpus, work (CPU workload)\r\n")
        }
        CommandKind::Echo(start) => {
            output.bytes(&parsed.line.bytes[start..parsed.line.len]);
            output.bytes(b"\r\n");
        }
        CommandKind::Uptime => {
            render_snapshot(telemetry.snapshot(CommandRequest::Uptime, 0), output)
        }
        CommandKind::Cpu => render_snapshot(telemetry.snapshot(CommandRequest::Cpu, 0), output),
        CommandKind::Memory => {
            render_snapshot(telemetry.snapshot(CommandRequest::Memory, 0), output)
        }
        CommandKind::Sched => render_snapshot(telemetry.snapshot(CommandRequest::Sched, 0), output),
        CommandKind::Cpus | CommandKind::Work => unreachable!(),
        CommandKind::Empty => {}
        CommandKind::Unknown => output.bytes(b"unknown command\r\n"),
    }
}

fn render_snapshot(snapshot: Option<Snapshot>, output: &mut Rendered<'_>) {
    match snapshot {
        Some(Snapshot::Uptime { seconds }) => {
            output.bytes(b"uptime: ");
            output.number(seconds as usize);
            output.bytes(b" seconds\r\n");
        }
        Some(Snapshot::Cpu { logical_id }) => {
            output.bytes(b"cpu: logical id ");
            output.number(logical_id as usize);
            output.bytes(b"\r\n");
        }
        Some(Snapshot::Memory {
            heap_used,
            page_used,
            capacity,
        }) => {
            output.bytes(b"memory: heap used ");
            output.number(heap_used);
            output.bytes(b" bytes; pages used ");
            output.number(page_used);
            output.bytes(b" bytes; capacity ");
            output.number(capacity);
            output.bytes(b" bytes\r\n");
        }
        Some(Snapshot::Sched { ready, injector }) => {
            output.bytes(b"sched: ready ");
            output.number(ready);
            output.bytes(b"; injector ");
            output.number(injector);
            output.bytes(b"\r\n");
        }
        Some(Snapshot::CpuState {
            logical_id,
            state,
            thread,
        }) => {
            output.bytes(b"cpu ");
            output.number(logical_id as usize);
            output.byte(b' ');
            output.bytes(match state {
                CpuState::NotReady => b"not ready",
                CpuState::Busy => b"busy",
                CpuState::Idle => b"idle",
                CpuState::Active => b"active",
            });

            if let Some(thread) = thread {
                output.bytes(b"; thread ");
                output.number(thread.id as usize);
                output.byte(b' ');
                output.bytes(match thread.state {
                    crate::monitor::ThreadState::Running => b"running",
                    crate::monitor::ThreadState::Ready => b"ready",
                    crate::monitor::ThreadState::Blocked => b"blocked",
                    crate::monitor::ThreadState::Dead => b"dead",
                });
                output.byte(b' ');
                output.bytes(match thread.kind {
                    crate::monitor::ThreadKind::Kernel => b"kernel",
                    crate::monitor::ThreadKind::User => b"user",
                });

                if thread.idle {
                    output.bytes(b" idle");
                }
            }

            output.bytes(b"\r\n");
        }
        None => output.bytes(b"telemetry busy or unavailable\r\n"),
    }
}

pub(crate) struct Rendered<'a> {
    output: &'a mut [u8],
    pub(crate) len: usize,
}

impl<'a> Rendered<'a> {
    pub(crate) fn new(output: &'a mut [u8]) -> Self {
        Self { output, len: 0 }
    }

    pub(crate) fn bytes(&mut self, bytes: &[u8]) {
        self.output[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
    }

    pub(crate) fn byte(&mut self, byte: u8) {
        self.output[self.len] = byte;
        self.len += 1;
    }

    fn number(&mut self, mut value: usize) {
        let mut digits = [0u8; 20];
        let mut len = 0;

        loop {
            digits[len] = b'0' + (value % 10) as u8;
            len += 1;
            value /= 10;

            if value == 0 {
                break;
            }
        }

        while len > 0 {
            len -= 1;
            self.byte(digits[len]);
        }
    }
}
