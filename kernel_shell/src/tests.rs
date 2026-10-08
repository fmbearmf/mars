use super::*;

fn send(shell: &mut Shell, input: &[u8], output: &mut [u8; OUTPUT_CAPACITY]) -> usize {
    let mut scratch = [0; OUTPUT_CAPACITY];
    let mut used = 0;
    for byte in input {
        let count = shell.feed(*byte, &mut scratch);
        output[used..used + count].copy_from_slice(&scratch[..count]);
        used += count;
    }

    used
}

#[test]
fn help_and_echo_commands() {
    let mut shell = Shell::new();
    let mut out = [0; OUTPUT_CAPACITY];

    let n = send(&mut shell, b"help\r", &mut out);
    assert_eq!(
        &out[..n],
        b"help\r\ncommands: help, echo <text>, uptime, cpu, memory, sched, cpus (thread details)\r\n> "
    );

    let n = send(&mut shell, b"echo hi\n", &mut out);
    assert_eq!(&out[..n], b"echo hi\r\nhi\r\n> ");
}

#[test]
fn crlf_executes_only_once() {
    let mut shell = Shell::new();
    let mut out = [0; OUTPUT_CAPACITY];

    let n = send(&mut shell, b"help\r\n", &mut out);
    assert_eq!(
        &out[..n],
        b"help\r\ncommands: help, echo <text>, uptime, cpu, memory, sched, cpus (thread details)\r\n> "
    );
}

#[test]
fn backspace_and_overflow_are_bounded() {
    let mut shell = Shell::new();
    let mut out = [0; OUTPUT_CAPACITY];

    let n = send(&mut shell, b"echox\x08o ok\r", &mut out);
    assert_eq!(&out[..n], b"echox\x08 \x08o ok\r\nunknown command\r\n> ");

    let mut input = [b'a'; LINE_CAPACITY + 2];
    input[LINE_CAPACITY + 1] = b'\n';

    let n = send(&mut shell, &input, &mut out);
    assert!(out[..n].ends_with(b"input too long\r\n> "));

    let mut input = [b'a'; LINE_CAPACITY + 3];
    input[LINE_CAPACITY + 1] = 8;
    input[LINE_CAPACITY + 2] = b'\n';

    let n = send(&mut shell, &input, &mut out);
    assert!(out[..n].ends_with(b"input too long\r\n> "));
    assert!(
        !out[..n]
            .windows(b"unknown command".len())
            .any(|window| window == b"unknown command")
    );
}

#[test]
fn input_error_discards_tail_and_recovers_from_crlf() {
    let mut shell = Shell::new();
    let mut out = [0; OUTPUT_CAPACITY];
    let mut scratch = [0; OUTPUT_CAPACITY];

    let n = send(&mut shell, b"help", &mut out);
    assert_eq!(&out[..n], b"help");

    let n = shell.input_error(&mut scratch);
    assert_eq!(&scratch[..n], b"input error; line discarded\r\n");

    let n = send(&mut shell, b"tail\r\nhelp\r\n", &mut out);
    assert_eq!(
        &out[..n],
        b"\r\n> help\r\ncommands: help, echo <text>, uptime, cpu, memory, sched, cpus (thread details)\r\n> "
    );
}

#[test]
fn overflow_recovers_from_crlf_without_echoing_tail() {
    let mut shell = Shell::new();
    let mut out = [0; OUTPUT_CAPACITY];

    let mut input = [b'a'; LINE_CAPACITY + 2];
    input[LINE_CAPACITY] = b't';
    input[LINE_CAPACITY + 1] = b'\r';

    let n = send(&mut shell, &input, &mut out);
    assert!(out[..n].ends_with(b"input too long\r\n> "));
    assert_eq!(&out[LINE_CAPACITY..LINE_CAPACITY + 2], b"\r\n");

    let n = send(&mut shell, b"\nhelp\r\n", &mut out);
    assert_eq!(
        &out[..n],
        b"help\r\ncommands: help, echo <text>, uptime, cpu, memory, sched, cpus (thread details)\r\n> "
    );
}

#[test]
fn capacity_control_and_empty_backspace_edges() {
    let mut line = Line::<Editing>::new();

    assert!(!line.backspace());

    for _ in 0..LINE_CAPACITY {
        assert!(line.push(b'a'));
    }

    assert!(!line.push(b'b'));
    assert!(line.backspace());
    assert!(line.push(b'b'));

    let mut shell = Shell::new();
    let mut out = [0; OUTPUT_CAPACITY];

    let n = send(&mut shell, b"\x01\x7f\x08\x1f\x7e\n", &mut out);
    assert_eq!(&out[..n], b"~\r\nunknown command\r\n> ");
}

#[test]
fn exact_capacity_input_and_maximum_echo_render_safely() {
    let mut shell = Shell::new();
    let mut out = [0; OUTPUT_CAPACITY];
    let input = [b'x'; LINE_CAPACITY];

    let n = send(&mut shell, &input, &mut out);
    assert_eq!(n, LINE_CAPACITY);
    assert_eq!(&out[..n], &input);

    let n = send(&mut shell, b"\n", &mut out);
    assert_eq!(&out[..n], b"\r\nunknown command\r\n> ");

    let mut command = [b'a'; LINE_CAPACITY];
    command[..5].copy_from_slice(b"echo ");

    let n = send(&mut shell, &command, &mut out);
    assert_eq!(n, LINE_CAPACITY);

    let n = send(&mut shell, b"\n", &mut out);
    assert_eq!(n, LINE_CAPACITY + 1);
    assert_eq!(&out[..2], b"\r\n");
    assert!(out[2..LINE_CAPACITY - 3].iter().all(|byte| *byte == b'a'));
    assert_eq!(&out[LINE_CAPACITY - 3..n], b"\r\n> ");
}

#[test]
fn monitoring_commands_are_parsed_as_distinct_commands() {
    for (input, expected) in [
        (b"uptime".as_slice(), CommandKind::Uptime),
        (b"cpu", CommandKind::Cpu),
        (b"memory", CommandKind::Memory),
        (b"sched", CommandKind::Sched),
        (b"cpus", CommandKind::Cpus),
    ] {
        let mut line = Line::<Editing>::new();
        for byte in input {
            line.push(*byte);
        }

        assert_eq!(parse(line.complete()).command, expected);
    }
}

#[test]
fn monitor_commands_render_typed_snapshots() {
    let mut shell = Shell::new();
    let mut out = [0; OUTPUT_CAPACITY];
    let mut telemetry = FixtureTelemetry;

    for (command, expected) in [
        (
            b"uptime\n".as_slice(),
            b"uptime\r\nuptime: 42 seconds\r\n> ".as_slice(),
        ),
        (b"cpu\n", b"cpu\r\ncpu: logical id 3\r\n> "),
        (
            b"memory\n",
            b"memory\r\nmemory: heap used 12 bytes; pages used 34 bytes; capacity 56 bytes\r\n> ",
        ),
        (b"sched\n", b"sched\r\nsched: ready 7; injector 2\r\n> "),
    ] {
        let n = send_streaming(&mut shell, command, &mut out, &mut telemetry);
        assert_eq!(&out[..n], expected);
    }
}

#[test]
fn cpus_render_thread_metadata_and_all_statuses() {
    let mut shell = Shell::new();
    let mut out = [0; OUTPUT_CAPACITY];
    let mut telemetry = FixtureTelemetry;

    let n = send_streaming(&mut shell, b"cpus\n", &mut out, &mut telemetry);
    assert_eq!(
        &out[..n],
        b"cpus\r\ncpu 0 idle; thread 11 running kernel idle\r\ncpu 1 active; thread 22 ready user\r\ncpu 2 not ready\r\ncpu 3 busy\r\n> "
    );
}

#[test]
fn cpus_streams_every_row_without_a_fixed_limit() {
    let mut shell = Shell::new();
    let mut telemetry = ManyCpus;
    let mut rows = 0;

    for byte in b"cpus\n" {
        shell.feed_streaming_with(*byte, &mut telemetry, &mut |output| {
            if output.starts_with(b"cpu ") {
                rows += 1;

                assert_eq!(
                    output,
                    b"cpu 4294967295 active; thread 4294967295 blocked kernel\r\n"
                );
            }
        });
    }

    assert_eq!(rows, ManyCpus::COUNT);
}

fn send_streaming<T: Telemetry>(
    shell: &mut Shell,
    input: &[u8],
    output: &mut [u8; OUTPUT_CAPACITY],
    telemetry: &mut T,
) -> usize {
    let mut used = 0;
    let mut scratch = [0; OUTPUT_CAPACITY];

    for byte in input {
        let mut count = 0;
        shell.feed_streaming_with(*byte, telemetry, &mut |bytes| {
            scratch[count..count + bytes.len()].copy_from_slice(bytes);
            count += bytes.len();
        });

        output[used..used + count].copy_from_slice(&scratch[..count]);
        used += count;
    }

    used
}

struct FixtureTelemetry;

impl Telemetry for FixtureTelemetry {
    fn snapshot(&mut self, request: CommandRequest, index: usize) -> Option<Snapshot> {
        Some(match request {
            CommandRequest::Uptime => Snapshot::Uptime { seconds: 42 },
            CommandRequest::Cpu => Snapshot::Cpu { logical_id: 3 },
            CommandRequest::Memory => Snapshot::Memory {
                heap_used: 12,
                page_used: 34,
                capacity: 56,
            },
            CommandRequest::Sched => Snapshot::Sched {
                ready: 7,
                injector: 2,
            },
            CommandRequest::Cpus => match index {
                0 => Snapshot::CpuState {
                    logical_id: 0,
                    state: CpuState::Idle,
                    thread: Some(ThreadMetadata {
                        id: 11,
                        state: ThreadState::Running,
                        kind: ThreadKind::Kernel,
                        idle: true,
                    }),
                },
                1 => Snapshot::CpuState {
                    logical_id: 1,
                    state: CpuState::Active,
                    thread: Some(ThreadMetadata {
                        id: 22,
                        state: ThreadState::Ready,
                        kind: ThreadKind::User,
                        idle: false,
                    }),
                },
                2 => Snapshot::CpuState {
                    logical_id: 2,
                    state: CpuState::NotReady,
                    thread: None,
                },
                3 => Snapshot::CpuState {
                    logical_id: 3,
                    state: CpuState::Busy,
                    thread: None,
                },
                _ => return None,
            },
        })
    }
}

struct ManyCpus;

impl ManyCpus {
    const COUNT: usize = 200;
}

impl Telemetry for ManyCpus {
    fn snapshot(&mut self, request: CommandRequest, index: usize) -> Option<Snapshot> {
        if request != CommandRequest::Cpus || index == Self::COUNT {
            return None;
        }

        Some(Snapshot::CpuState {
            logical_id: u32::MAX,
            state: CpuState::Active,
            thread: Some(ThreadMetadata {
                id: u32::MAX,
                state: ThreadState::Blocked,
                kind: ThreadKind::Kernel,
                idle: false,
            }),
        })
    }
}

#[test]
fn parser_requires_completed_line() {
    let mut line = Line::<Editing>::new();

    line.push(b'h');
    line.push(b'e');
    line.push(b'l');
    line.push(b'p');

    assert_eq!(parse(line.complete()).command, CommandKind::Help);
}
