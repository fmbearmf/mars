pub(crate) const LINE_CAPACITY: usize = 128;

/// a line that is being edited or has crossed a validated protocol transition
pub struct Line<State> {
    pub(crate) bytes: [u8; LINE_CAPACITY],
    pub(crate) len: usize,
    pub(crate) state: core::marker::PhantomData<State>,
}

/// a line that accepts printable input and editing operations
pub struct Editing;
/// a line that has been terminated and is ready to parse
pub struct Complete;
/// a line that has been parsed
pub struct Parsed;

impl Line<Editing> {
    /// create a valid, empty editing line
    pub const fn new() -> Self {
        Self {
            bytes: [0; LINE_CAPACITY],
            len: 0,
            state: core::marker::PhantomData,
        }
    }

    /// append a byte, returning false when the line has reached capacity
    pub fn push(&mut self, byte: u8) -> bool {
        if self.len == LINE_CAPACITY {
            return false;
        }
        self.bytes[self.len] = byte;
        self.len += 1;
        true
    }

    /// remove one byte if present
    pub fn backspace(&mut self) -> bool {
        if self.len == 0 {
            false
        } else {
            self.len -= 1;
            true
        }
    }

    /// finish editing and make the line available to the parser
    pub fn complete(self) -> Line<Complete> {
        Line {
            bytes: self.bytes,
            len: self.len,
            state: core::marker::PhantomData,
        }
    }
}

impl Default for Line<Editing> {
    fn default() -> Self {
        Self::new()
    }
}

impl<State> Line<State> {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}
