use crate::{dma::DmaBuffer, trb::Trb};

pub struct CommandRing {
    buffer: DmaBuffer<[Trb]>,
    cycle_state: bool,
    enqueue_index: usize,
    capacity: usize,
}

impl CommandRing {
    pub fn new(capacity: usize) -> Result<Self, &'static str> {
        if capacity < 2 {
            return Err("cmd ring capacity must be >= 2");
        }

        let mut buffer = DmaBuffer::new_slice(capacity, Trb::empty())?;
        let phys = buffer.phys_addr();

        // place link TRB at the end pointing back to ring base with Toggle Cycle [1] set
        buffer.write_volatile(capacity - 1, Trb::link(phys, true));
        buffer.sync_for_device();

        Ok(Self {
            buffer,
            cycle_state: true,
            enqueue_index: 0,
            capacity,
        })
    }

    pub fn phys_addr(&self) -> u64 {
        self.buffer.phys_addr()
    }

    pub fn enqueue(&mut self, mut trb: Trb) -> Result<u64, &'static str> {
        if self.cycle_state {
            trb.control |= 1;
        } else {
            trb.control &= !1;
        }

        let trb_phys = self.buffer.phys_addr() + (self.enqueue_index * size_of::<Trb>()) as u64;
        self.buffer.write_volatile(self.enqueue_index, trb);
        self.enqueue_index += 1;

        if self.enqueue_index == self.capacity - 1 {
            let mut link = self.buffer.read_volatile(self.enqueue_index);
            if self.cycle_state {
                link.control |= 1;
            } else {
                link.control &= !1;
            }
            self.buffer.write_volatile(self.enqueue_index, link);

            self.enqueue_index = 0;
            self.cycle_state = !self.cycle_state;
        }

        self.buffer.sync_for_device();
        Ok(trb_phys)
    }
}

pub struct TransferRing {
    buffer: DmaBuffer<[Trb]>,
    cycle_state: bool,
    enqueue_index: usize,
    capacity: usize,
}

impl TransferRing {
    pub fn new(capacity: usize) -> Result<Self, &'static str> {
        if capacity < 2 {
            return Err("transfer ring capacity must be >= 2");
        }

        let mut buffer = DmaBuffer::new_slice(capacity, Trb::empty())?;
        let phys = buffer.phys_addr();
        buffer.write_volatile(capacity - 1, Trb::link(phys, true));
        buffer.sync_for_device();

        Ok(Self {
            buffer,
            cycle_state: true,
            enqueue_index: 0,
            capacity,
        })
    }

    pub fn phys_addr(&self) -> u64 {
        self.buffer.phys_addr()
    }

    pub fn enqueue(&mut self, mut trb: Trb) -> Result<u64, &'static str> {
        if self.cycle_state {
            trb.control |= 1;
        } else {
            trb.control &= !1;
        }

        let trb_phys = self.buffer.phys_addr() + (self.enqueue_index * size_of::<Trb>()) as u64;
        self.buffer.write_volatile(self.enqueue_index, trb);
        self.enqueue_index += 1;

        if self.enqueue_index == self.capacity - 1 {
            let mut link = self.buffer.read_volatile(self.enqueue_index);
            if self.cycle_state {
                link.control |= 1;
            } else {
                link.control &= !1;
            }
            self.buffer.write_volatile(self.enqueue_index, link);
            self.enqueue_index = 0;
            self.cycle_state = !self.cycle_state;
        }

        self.buffer.sync_for_device();
        Ok(trb_phys)
    }

    pub fn dequeue_pointer(&self) -> u64 {
        self.buffer.phys_addr() + (self.enqueue_index * size_of::<Trb>()) as u64
            | u64::from(self.cycle_state)
    }
}

pub struct EventRing {
    pub buffer: DmaBuffer<[Trb]>,
    cycle_state: bool,
    dequeue_index: usize,
}

impl EventRing {
    pub fn new(capacity: usize) -> Result<Self, &'static str> {
        let buffer = DmaBuffer::new_slice(capacity, Trb::empty())?;
        buffer.sync_for_device();
        Ok(Self {
            buffer,
            cycle_state: true,
            dequeue_index: 0,
        })
    }

    pub fn phys_addr(&self) -> u64 {
        self.buffer.phys_addr()
    }

    pub fn capacity(&self) -> usize {
        self.buffer.len()
    }

    pub fn next_event(&mut self) -> Option<Trb> {
        self.buffer.sync_for_cpu();
        let trb = self.buffer.read_volatile(self.dequeue_index);
        if trb.is_cycle_state(self.cycle_state) {
            self.dequeue_index += 1;
            if self.dequeue_index == self.buffer.len() {
                self.dequeue_index = 0;
                self.cycle_state = !self.cycle_state;
            }
            Some(trb)
        } else {
            None
        }
    }

    pub fn current_erdp(&self) -> u64 {
        self.phys_addr() + (self.dequeue_index * size_of::<Trb>()) as u64
    }
}

pub struct PendingEvents<'a> {
    buffer: &'a [Trb],
    index: usize,
    cycle: bool,
}

impl<'a> Iterator for PendingEvents<'a> {
    type Item = &'a Trb;

    fn next(&mut self) -> Option<Self::Item> {
        let trb = &self.buffer[self.index];

        if trb.is_cycle_state(self.cycle) {
            self.index += 1;
            if self.index == self.buffer.len() {
                self.index = 0;
                self.cycle = !self.cycle;
            }
            Some(trb)
        } else {
            None
        }
    }
}
