use crate::command::parse;
use crate::line::{Complete, Editing, Line};
use crate::monitor::{Snapshot, Telemetry};
use crate::render::{OUTPUT_CAPACITY, Rendered, render_command_stream};

pub const PROMPT: &[u8] = b"-> ";

/// owns the active line-editing state and emits terminal feedback and command results
pub struct Shell {
    state: InputState,
}

impl Shell {
    pub const fn new() -> Self {
        Self {
            state: InputState::Editing(Line::new()),
        }
    }

    /// consume one UART byte and write simple terminal output to `output`
    pub fn feed(&mut self, byte: u8, output: &mut [u8; OUTPUT_CAPACITY]) -> usize {
        let mut used = 0;
        self.feed_streaming_with(byte, &mut NoTelemetry, &mut |bytes| {
            output[used..used + bytes.len()].copy_from_slice(bytes);
            used += bytes.len();
        });
        used
    }

    /// consume one UART byte and stream terminal output to `emit`
    ///
    /// monitoring output is emitted in chunks so CPU rows are not accumulated in a fixed-size buffer
    pub fn feed_streaming_with<T: Telemetry>(
        &mut self,
        byte: u8,
        telemetry: &mut T,
        emit: &mut impl FnMut(&[u8]),
    ) {
        let state = core::mem::replace(&mut self.state, InputState::Editing(Line::new()));
        let (state, event) = state.consume(byte);

        self.state = state;

        match event {
            InputEvent::Echo(byte) => emit(&[byte]),
            InputEvent::Erase => emit(b"\x08 \x08"),
            InputEvent::Complete(line, suppress_lf) => {
                emit(b"\r\n");
                render_command_stream(parse(line), telemetry, emit);
                emit(PROMPT);

                self.state = if suppress_lf {
                    InputState::SuppressLf(Line::new())
                } else {
                    InputState::Editing(Line::new())
                };
            }
            InputEvent::Overflow(suppress_lf) => {
                emit(b"\r\ninput too long\r\n");
                emit(PROMPT);

                self.state = if suppress_lf {
                    InputState::SuppressLf(Line::new())
                } else {
                    InputState::Editing(Line::new())
                };
            }
            InputEvent::Discarded(suppress_lf) => {
                emit(b"\r\n");
                emit(PROMPT);

                self.state = if suppress_lf {
                    InputState::SuppressLf(Line::new())
                } else {
                    InputState::Editing(Line::new())
                };
            }
            InputEvent::None => {}
        }
    }

    /// discard the current line after an input error and report the loss
    pub fn input_error(&mut self, output: &mut [u8; OUTPUT_CAPACITY]) -> usize {
        self.state = InputState::Discarding;

        let mut rendered = Rendered::new(output);

        rendered.bytes(b"input error; line discarded\r\n");
        rendered.len
    }
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

enum InputState {
    Editing(Line<Editing>),
    Overflowed,
    Discarding,
    SuppressLf(Line<Editing>),
}

enum InputEvent {
    None,
    Echo(u8),
    Erase,
    Complete(Line<Complete>, bool),
    Overflow(bool),
    Discarded(bool),
}

impl InputState {
    fn consume(self, byte: u8) -> (Self, InputEvent) {
        if let Self::SuppressLf(line) = self {
            if byte == b'\n' {
                return (Self::Editing(line), InputEvent::None);
            }

            return Self::Editing(line).consume_active(byte);
        }

        self.consume_active(byte)
    }

    fn consume_active(self, byte: u8) -> (Self, InputEvent) {
        match self {
            Self::Editing(mut line) => match byte {
                b'\r' | b'\n' => (
                    Self::Editing(Line::new()),
                    InputEvent::Complete(line.complete(), byte == b'\r'),
                ),
                8 | 127 if line.backspace() => (Self::Editing(line), InputEvent::Erase),
                0x20..=0x7e => {
                    if line.push(byte) {
                        (Self::Editing(line), InputEvent::Echo(byte))
                    } else {
                        (Self::Overflowed, InputEvent::None)
                    }
                }
                _ => (Self::Editing(line), InputEvent::None),
            },
            Self::Overflowed => match byte {
                b'\r' | b'\n' => (
                    Self::Editing(Line::new()),
                    InputEvent::Overflow(byte == b'\r'),
                ),
                _ => (Self::Overflowed, InputEvent::None),
            },
            Self::Discarding => match byte {
                b'\r' | b'\n' => (
                    Self::Editing(Line::new()),
                    InputEvent::Discarded(byte == b'\r'),
                ),
                _ => (Self::Discarding, InputEvent::None),
            },
            Self::SuppressLf(_) => unreachable!(),
        }
    }
}

struct NoTelemetry;

impl Telemetry for NoTelemetry {
    fn snapshot(&mut self, _: crate::monitor::CommandRequest, _: usize) -> Option<Snapshot> {
        None
    }
}
