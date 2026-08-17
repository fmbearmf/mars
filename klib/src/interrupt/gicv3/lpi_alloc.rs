use alloc::{vec, vec::Vec};

pub struct LpiAllocator {
    bitmap: Vec<u64>,
    base_id: u32,
    max_id: u32,
    hint: usize,
}

impl LpiAllocator {
    pub fn new(base_id: u32, max_id: u32) -> Self {
        let count = max_id - base_id + 1;
        let words = ((count + 63) / 64) as usize;
        Self {
            bitmap: vec![0; words],
            base_id,
            max_id,
            hint: 0,
        }
    }

    pub fn alloc(&mut self) -> Option<u32> {
        let words = self.bitmap.len();
        for i in 0..words {
            let i = (self.hint + i) % words;
            let word = self.bitmap[i];

            if word != !0 {
                // if not all bits are 1
                let bit = (!word).trailing_zeros() as usize;
                self.bitmap[i] |= 1u64 << bit;
                self.hint = i;

                let id = self.base_id + (i as u32 * 64) + bit as u32;
                if id > self.max_id {
                    self.bitmap[i] &= !(1u64 << bit);
                    continue;
                }
                return Some(id);
            }
        }
        None
    }

    pub fn reserve(&mut self, id: u32) -> Result<(), ()> {
        if !(self.base_id..=self.max_id).contains(&id) {
            return Err(());
        }

        let offset = id - self.base_id;
        let i = (offset / 64) as usize;
        let bit = offset % 64;

        if (self.bitmap[i] & (1u64 << bit)) != 0 {
            return Err(()); // in use
        }

        self.bitmap[i] |= 1u64 << bit;

        Ok(())
    }

    pub fn free(&mut self, id: u32) {
        if !(self.base_id..=self.max_id).contains(&id) {
            return;
        }

        let offset = id - self.base_id;
        let i = (offset / 64) as usize;
        let bit = offset % 64;

        self.bitmap[i] &= !(1u64 << bit);

        // move hint backwards for denser packing of smaller LPIs
        if i < self.hint {
            self.hint = i;
        }
    }
}
