use alloc::vec::Vec;
use core::arch::asm;

use klib::{
    block::{BlockError, Result as BlockResult},
    vm::PAGE_SIZE,
};

use crate::{
    dma::{DmaPage, barrier},
    protocol::{Command, Completion, ControllerIdentify, IoError, NamespaceIdentify, validate_io},
    registers::Registers,
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct NamespaceInfo {
    pub nsid: u32,
    pub block_size: usize,
    pub block_count: u64,
}

struct Queue {
    sq: DmaPage,
    cq: DmaPage,
    depth: u16,
    tail: u16,
    head: u16,
    phase: bool,
    cid: u16,
    id: u16,
}

#[derive(Debug)]
enum CommandError {
    Status(u16),
    Timeout,
    Fatal,
    InvalidCompletion,
    Offline,
}
impl CommandError {
    fn message(&self) -> &'static str {
        match self {
            Self::Status(_) => "NVMe command failed",
            Self::Timeout => "NVMe command timed out",
            Self::Fatal => "NVMe controller fatal status",
            Self::InvalidCompletion => "malformed NVMe completion",
            Self::Offline => "NVMe controller is offline",
        }
    }
}

pub(crate) struct Controller {
    regs: Registers,
    admin: Queue,
    io: Queue,
    bounce: DmaPage,
    prps: DmaPage,
    stride: usize,
    command_timeout: u64,
    ready_timeout: u64,
    page_bytes: usize,
    transfer_bytes: usize,
    faulted: bool,
    stopped: bool,
    #[cfg(feature = "self-test")]
    serial: [u8; 20],
}

impl Controller {
    pub(crate) fn initialize(
        mut regs: Registers,
        address_bits: u8,
        enable_dma: impl FnOnce(),
    ) -> Result<(Self, Vec<NamespaceInfo>), &'static str> {
        if regs.len() < 0x1008 {
            return Err("NVMe register aperture is too short");
        }
        let cap = regs.read64(0);
        if cap == u64::MAX || cap & (1 << 37) == 0 {
            return Err("controller lacks the NVM command set");
        }
        let mps = ((cap >> 48) & 0xf) as u32;
        let maximum_mps = ((cap >> 52) & 0xf) as u32;
        if mps > maximum_mps || mps > PAGE_SIZE.trailing_zeros() - 12 {
            return Err("unsupported NVMe memory page size");
        }
        let page_bytes = 4096usize << mps;
        let stride = 4usize << ((cap >> 32) & 0xf);
        if 0x1000 + 3 * stride + 4 > regs.len() {
            return Err("BAR0 omits I/O queue doorbells");
        }
        let depth = ((cap & 0xffff) + 1).min(64).min((page_bytes / 64) as u64) as u16;
        if depth < 2 {
            return Err("controller queue depth is less than two");
        }
        let frequency = timer_frequency();
        if frequency == 0 || frequency > u64::from(u32::MAX) {
            return Err("invalid architectural timer frequency");
        }
        let ready_timeout = frequency * (((cap >> 24) & 0xff) + 1) / 2;
        let command_timeout = frequency * 30;
        regs.write32(0x0c, u32::MAX);
        // Wait even when EN is already clear: firmware may have initiated a reset.
        regs.write32(0x14, regs.read32(0x14) & !((3 << 14) | 1));
        if !wait_register(&regs, 0x1c, 1, 0, ready_timeout) {
            return Err("controller did not disable");
        }

        let mut controller = Self {
            regs,
            admin: Queue::new(address_bits, depth, 0)?,
            io: Queue::new(address_bits, depth, 1)?,
            bounce: DmaPage::allocate(address_bits)?,
            prps: DmaPage::allocate(address_bits)?,
            stride,
            command_timeout,
            ready_timeout,
            page_bytes,
            transfer_bytes: PAGE_SIZE,
            faulted: false,
            stopped: true,
            #[cfg(feature = "self-test")]
            serial: [0; 20],
        };
        controller.admin.prepare();
        controller.io.prepare();
        controller.bounce.prepare_for_device();
        controller.prps.prepare_for_device();
        controller.admin.pin();
        controller.io.pin();
        controller.bounce.pin();
        controller.prps.pin();
        controller.stopped = false;
        controller.regs.write32(
            0x24,
            ((u32::from(depth) - 1) << 16) | (u32::from(depth) - 1),
        );
        controller.regs.write64(0x28, controller.admin.sq.phys());
        controller.regs.write64(0x30, controller.admin.cq.phys());
        let cc = (mps << 7) | (6 << 16) | (4 << 20);
        controller.regs.write32(0x14, cc);
        let configured_cap = controller.regs.read64(0);
        let mut ready_units = (configured_cap >> 24) & 0xff;
        if configured_cap & (1 << 59) != 0 {
            ready_units = ready_units.max(u64::from(controller.regs.read32(0x68) & 0xffff));
        }
        controller.ready_timeout = frequency * (ready_units + 1) / 2;
        enable_dma();
        controller.regs.write32(0x14, cc | 1);
        if !wait_register(&controller.regs, 0x1c, 1, 1, controller.ready_timeout) {
            controller.faulted = true;
            return Err("controller did not become ready");
        }
        let identify = controller.identify(0, 1).map_err(|error| error.message())?;
        let id = ControllerIdentify::parse(&identify);
        #[cfg(feature = "self-test")]
        controller.serial.copy_from_slice(&identify[4..24]);
        if identify[512] & 0xf > 6
            || identify[512] >> 4 < 6
            || identify[513] & 0xf > 4
            || identify[513] >> 4 < 4
        {
            return Err("unsupported NVMe queue entry sizes");
        }
        if id.mdts != 0 && u32::from(id.mdts) < (PAGE_SIZE / page_bytes).trailing_zeros() {
            controller.transfer_bytes = page_bytes << id.mdts;
        }
        controller
            .admin_command(Command::set_number_of_queues(0))
            .map_err(|error| error.message())?;
        let cq = Command::create_cq(0, 1, u32::from(depth), controller.io.cq.phys(), None)
            .map_err(|_| "invalid I/O completion queue")?;
        controller
            .admin_command(cq)
            .map_err(|error| error.message())?;
        let sq = Command::create_sq(0, 1, u32::from(depth), controller.io.sq.phys(), 1)
            .map_err(|_| "invalid I/O submission queue")?;
        controller
            .admin_command(sq)
            .map_err(|error| error.message())?;

        let mut namespaces = Vec::new();
        let mut cursor = 0u32;
        let mut count = 0u32;
        loop {
            let list = controller
                .identify(cursor, 2)
                .map_err(|error| error.message())?;
            let mut full = true;
            let mut terminated = false;
            for chunk in list.chunks_exact(4) {
                let nsid = u32::from_le_bytes(chunk.try_into().unwrap());
                if nsid == 0 {
                    terminated = true;
                    full = false;
                    continue;
                }
                if terminated || nsid <= cursor || nsid == u32::MAX || count >= id.namespace_count {
                    return Err("malformed active namespace list");
                }
                cursor = nsid;
                count += 1;
                let data = match controller.identify(nsid, 0) {
                    Ok(data) => data,
                    Err(CommandError::Status(status)) => {
                        log::warn!("nvme: namespace {nsid} Identify status {status:#x}");
                        continue;
                    }
                    Err(error) => return Err(error.message()),
                };
                match NamespaceIdentify::parse(&data) {
                    Ok(ns) if ns.block_size as usize <= controller.transfer_bytes => {
                        namespaces
                            .try_reserve(1)
                            .map_err(|_| "namespace allocation failed")?;
                        namespaces.push(NamespaceInfo {
                            nsid,
                            block_size: ns.block_size as usize,
                            block_count: ns.block_count,
                        });
                    }
                    other => log::warn!("nvme: namespace {nsid} unsupported: {other:?}"),
                }
            }
            if !full || count == id.namespace_count {
                break;
            }
        }
        if namespaces.is_empty() {
            return Err("controller has no supported active namespaces");
        }
        log::info!(
            "nvme: ready, {} namespace(s), queue depth {depth}, controller page {page_bytes}, transfer limit {}",
            namespaces.len(),
            controller.transfer_bytes
        );
        Ok((controller, namespaces))
    }

    #[cfg(feature = "self-test")]
    pub(crate) fn is_test_device(&self) -> bool {
        &self.serial == b"MARS_NVME_TEST      " || self.is_timeout_test()
    }

    #[cfg(feature = "self-test")]
    pub(crate) fn is_timeout_test(&self) -> bool {
        &self.serial == b"MARS_NVME_TIMEOUT   "
    }

    #[cfg(feature = "self-test")]
    pub(crate) fn test_invalid_opcode(&mut self, nsid: u32) -> BlockResult<()> {
        let mut command = Command::default();
        command.words[0] = 0xff;
        command.words[1] = nsid;
        match self.io_command(command) {
            Err(CommandError::Status(status)) if status & 0x7ff == 1 && !self.faulted => Ok(()),
            _ => Err(BlockError::HardwareError),
        }
    }

    #[cfg(feature = "self-test")]
    pub(crate) fn test_timeout(&mut self) -> BlockResult<()> {
        // An Async Event Request stays outstanding in the quiescent test device.
        let timeout = self.command_timeout;
        self.command_timeout = (timer_frequency() / 10).max(1);
        let mut command = Command::default();
        command.words[0] = 0x0c;
        let result = self.admin_command(command);
        self.command_timeout = timeout;
        if matches!(result, Err(CommandError::Timeout))
            && self.faulted
            && self.stopped
            && self.flush(1) == Err(BlockError::NotReady)
        {
            Ok(())
        } else {
            Err(BlockError::HardwareError)
        }
    }

    pub(crate) fn read_blocks(
        &mut self,
        nsid: u32,
        block_size: usize,
        block_count: u64,
        lba: u64,
        buf: &mut [u8],
    ) -> BlockResult<()> {
        validate_io(lba, buf.len(), block_size, block_count).map_err(block_error)?;
        if self.faulted {
            return Err(BlockError::NotReady);
        }
        let chunk_bytes = self.transfer_bytes / block_size * block_size;
        if chunk_bytes == 0 {
            return Err(BlockError::InvalidBlockSize);
        }
        let mut current_lba = lba;
        for chunk in buf.chunks_mut(chunk_bytes) {
            self.bounce.prepare_for_device();
            let prp2 = self.prepare_prps(chunk.len());
            let command = Command::read_write(
                0,
                nsid,
                current_lba,
                (chunk.len() / block_size) as u32,
                self.bounce.phys(),
                prp2,
                false,
            )
            .map_err(block_error)?;
            self.io_command(command)
                .map_err(|_| BlockError::HardwareError)?;
            self.bounce.invalidate_for_cpu();
            self.bounce.copy_to(chunk);
            current_lba += (chunk.len() / block_size) as u64;
        }
        Ok(())
    }

    pub(crate) fn write_blocks(
        &mut self,
        nsid: u32,
        block_size: usize,
        block_count: u64,
        lba: u64,
        buf: &[u8],
    ) -> BlockResult<()> {
        validate_io(lba, buf.len(), block_size, block_count).map_err(block_error)?;
        if self.faulted {
            return Err(BlockError::NotReady);
        }
        let chunk_bytes = self.transfer_bytes / block_size * block_size;
        if chunk_bytes == 0 {
            return Err(BlockError::InvalidBlockSize);
        }
        let mut current_lba = lba;
        for chunk in buf.chunks(chunk_bytes) {
            self.bounce.copy_from(chunk);
            self.bounce.prepare_for_device();
            let prp2 = self.prepare_prps(chunk.len());
            let command = Command::read_write(
                0,
                nsid,
                current_lba,
                (chunk.len() / block_size) as u32,
                self.bounce.phys(),
                prp2,
                true,
            )
            .map_err(block_error)?;
            self.io_command(command)
                .map_err(|_| BlockError::HardwareError)?;
            current_lba += (chunk.len() / block_size) as u64;
        }
        Ok(())
    }

    pub(crate) fn flush(&mut self, nsid: u32) -> BlockResult<()> {
        if self.faulted {
            return Err(BlockError::NotReady);
        }
        let command = Command::flush(0, nsid).map_err(block_error)?;
        self.io_command(command)
            .map(|_| ())
            .map_err(|_| BlockError::HardwareError)
    }

    fn prepare_prps(&mut self, bytes: usize) -> u64 {
        if bytes <= self.page_bytes {
            return 0;
        }
        if bytes <= 2 * self.page_bytes {
            return self.bounce.phys() + self.page_bytes as u64;
        }
        for (index, offset) in (self.page_bytes..bytes)
            .step_by(self.page_bytes)
            .enumerate()
        {
            let address = self.bounce.phys() + offset as u64;
            self.prps.write_u32(index * 8, address as u32);
            self.prps.write_u32(index * 8 + 4, (address >> 32) as u32);
        }
        self.prps.prepare_for_device();
        self.prps.phys()
    }

    fn identify(&mut self, nsid: u32, cns: u8) -> Result<[u8; 4096], CommandError> {
        self.bounce.prepare_for_device();
        self.admin_command(Command::identify(0, nsid, cns, self.bounce.phys()))?;
        self.bounce.invalidate_for_cpu();
        let mut data = [0u8; 4096];
        self.bounce.copy_to(&mut data);
        Ok(data)
    }

    fn admin_command(&mut self, command: Command) -> Result<u32, CommandError> {
        if self.faulted {
            return Err(CommandError::Offline);
        }
        let result = execute(
            &mut self.regs,
            &mut self.admin,
            self.stride,
            command,
            self.command_timeout,
        );
        self.check_result(result)
    }

    fn io_command(&mut self, command: Command) -> Result<u32, CommandError> {
        if self.faulted {
            return Err(CommandError::Offline);
        }
        let result = execute(
            &mut self.regs,
            &mut self.io,
            self.stride,
            command,
            self.command_timeout,
        );
        self.check_result(result)
    }

    fn check_result(&mut self, result: Result<u32, CommandError>) -> Result<u32, CommandError> {
        if let Err(error) = &result {
            log::error!("nvme: {error:?}");
            if !matches!(error, CommandError::Status(_)) {
                self.faulted = true;
                self.stop();
            }
        }
        result
    }

    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.regs
            .write32(0x14, self.regs.read32(0x14) & !((3 << 14) | 1));
        if wait_register(&self.regs, 0x1c, 1, 0, self.ready_timeout) {
            barrier();
            // RDY = 0 after clearing EN completes the controller reset (queue DMA too)
            unsafe {
                self.admin.sq.unpin_after_dma_stopped();
                self.admin.cq.unpin_after_dma_stopped();
                self.io.sq.unpin_after_dma_stopped();
                self.io.cq.unpin_after_dma_stopped();
                self.bounce.unpin_after_dma_stopped();
                self.prps.unpin_after_dma_stopped();
            }
            self.stopped = true;
        }
    }

    pub(crate) fn shutdown(&mut self) -> BlockResult<()> {
        if self.stopped {
            return Ok(());
        }
        let mut result = Ok(());
        if !self.faulted && self.regs.read32(0x1c) & 3 == 1 {
            self.regs
                .write32(0x14, (self.regs.read32(0x14) & !(3 << 14)) | (1 << 14));
            if !wait_register(&self.regs, 0x1c, 3 << 2, 2 << 2, self.command_timeout) {
                result = Err(BlockError::HardwareError);
            }
        }
        self.faulted = true;
        self.stop();
        if !self.stopped {
            result = Err(BlockError::HardwareError);
        }
        result
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        if self.shutdown().is_err() {
            log::error!("nvme: shutdown incomplete; DMA pages remain quarantined");
        }
    }
}

impl Queue {
    fn new(bits: u8, depth: u16, id: u16) -> Result<Self, &'static str> {
        Ok(Self {
            sq: DmaPage::allocate(bits)?,
            cq: DmaPage::allocate(bits)?,
            depth,
            tail: 0,
            head: 0,
            phase: true,
            cid: 0,
            id,
        })
    }
    fn prepare(&mut self) {
        self.sq.prepare_for_device();
        self.cq.prepare_for_device();
    }
    fn pin(&mut self) {
        self.sq.pin();
        self.cq.pin();
    }
}

fn execute(
    regs: &mut Registers,
    queue: &mut Queue,
    stride: usize,
    mut command: Command,
    timeout: u64,
) -> Result<u32, CommandError> {
    if regs.read32(0x1c) & 3 != 1 {
        return Err(CommandError::Fatal);
    }
    let cid = queue.cid;
    queue.cid = queue.cid.wrapping_add(1);
    command.words[0] = (command.words[0] & 0xffff) | (u32::from(cid) << 16);
    let offset = usize::from(queue.tail) * 64;
    for (index, word) in command.words.iter().copied().enumerate() {
        queue.sq.write_u32(offset + index * 4, word);
    }
    queue.sq.prepare_for_device();
    queue.tail = (queue.tail + 1) % queue.depth;
    let doorbell = 0x1000 + 2 * usize::from(queue.id) * stride;
    regs.write32(doorbell, u32::from(queue.tail));
    let start = timer_counter();
    loop {
        queue.cq.invalidate_for_cpu();
        let offset = usize::from(queue.head) * 16;
        let status = queue.cq.read_u32(offset + 12);
        if ((status >> 16) & 1 != 0) == queue.phase {
            queue.cq.invalidate_for_cpu();
            if queue.cq.read_u32(offset + 12) != status {
                return Err(CommandError::InvalidCompletion);
            }
            let completion = Completion::parse([
                queue.cq.read_u32(offset),
                queue.cq.read_u32(offset + 4),
                queue.cq.read_u32(offset + 8),
                status,
            ]);
            if completion.sq_id != queue.id
                || completion.cid != cid
                || completion.sq_head != queue.tail
            {
                return Err(CommandError::InvalidCompletion);
            }
            queue.head = (queue.head + 1) % queue.depth;
            if queue.head == 0 {
                queue.phase = !queue.phase;
            }
            regs.write32(doorbell + stride, u32::from(queue.head));
            if !completion.is_success() {
                log::error!(
                    "nvme: opcode {:#x} nsid {} completion status {:#x}",
                    command.words[0] & 0xff,
                    command.words[1],
                    completion.status
                );
                return Err(CommandError::Status(completion.status));
            }
            return Ok(completion.result);
        }
        if regs.read32(0x1c) & 3 != 1 {
            return Err(CommandError::Fatal);
        }
        if timer_counter().wrapping_sub(start) >= timeout {
            return Err(CommandError::Timeout);
        }
        core::hint::spin_loop();
    }
}

fn block_error(error: IoError) -> BlockError {
    match error {
        IoError::UnalignedBuffer => BlockError::UnalignedBuffer,
        IoError::InvalidBlockSize => BlockError::InvalidBlockSize,
        _ => BlockError::OutOfBounds,
    }
}

fn wait_register(regs: &Registers, offset: usize, mask: u32, wanted: u32, timeout: u64) -> bool {
    let start = timer_counter();
    loop {
        let value = regs.read32(offset);
        if value == u32::MAX {
            return false;
        }
        if value & mask == wanted {
            return true;
        }
        if timer_counter().wrapping_sub(start) >= timeout {
            return false;
        }
        core::hint::spin_loop();
    }
}
fn timer_counter() -> u64 {
    let value;
    unsafe {
        asm!("isb", "mrs {value}, cntpct_el0", value = out(reg) value, options(nostack, preserves_flags))
    };
    value
}
fn timer_frequency() -> u64 {
    let value;
    unsafe {
        asm!("mrs {value}, cntfrq_el0", value = out(reg) value, options(nomem, nostack, preserves_flags))
    };
    value
}
