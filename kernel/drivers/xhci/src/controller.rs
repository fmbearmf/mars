use alloc::collections::vec_deque::VecDeque;
use core::arch::asm;

use crate::{
    dma::DmaBuffer,
    registers::{ErstEntry, Volatile, XhciRegisters},
    ring::{CommandRing, EventRing},
    trb::{Trb, TrbType},
    usb::{MAX_TRANSFER_SIZE, SetupPacket, UsbDevice, UsbEndpoint, UsbSpeed},
};

const USBCMD_RUN: u32 = 1 << 0;
const USBCMD_HCRST: u32 = 1 << 1;
const USBCMD_INTE: u32 = 1 << 2;
const USBSTS_HCH: u32 = 1 << 0;
const USBSTS_HSE: u32 = 1 << 2;
const USBSTS_EINT: u32 = 1 << 3;
const USBSTS_PCD: u32 = 1 << 4;
const USBSTS_CNR: u32 = 1 << 11;
const HCCPARAMS1_CSZ: u32 = 1 << 2;
const HCCPARAMS1_PPC: u32 = 1 << 3;
const PORTSC_CCS: u32 = 1 << 0;
const PORTSC_PED: u32 = 1 << 1;
const PORTSC_PR: u32 = 1 << 4;
const PORTSC_PP: u32 = 1 << 9;
const PORTSC_SPEED_MASK: u32 = 0x0f << 10;
const PORTSC_SPEED_SHIFT: u32 = 10;
const PORTSC_PIC_MASK: u32 = 0x03 << 14;
const PORTSC_CSC: u32 = 1 << 17;
const PORTSC_PEC: u32 = 1 << 18;
const PORTSC_WRC: u32 = 1 << 19;
const PORTSC_OCC: u32 = 1 << 20;
const PORTSC_PRC: u32 = 1 << 21;
const PORTSC_PLC: u32 = 1 << 22;
const PORTSC_CEC: u32 = 1 << 23;
const PORTSC_WPR: u32 = 1 << 31;
const PORTSC_CHANGE_MASK: u32 =
    PORTSC_CSC | PORTSC_PEC | PORTSC_WRC | PORTSC_OCC | PORTSC_PRC | PORTSC_PLC | PORTSC_CEC;
const PORTSC_CONFIGURATION_MASK: u32 = PORTSC_PP | PORTSC_PIC_MASK;
const COMMAND_TIMEOUT_MS: u64 = 2000;
const TRANSFER_TIMEOUT_MS: u64 = 5000;
const PORT_RESET_TIMEOUT_MS: u64 = 1000;
const FALLBACK_POLL_LIMIT: usize = 100_000_000;

pub struct HostController {
    pub regs: XhciRegisters,
    pub dcbaa: DmaBuffer<[u64]>,
    pub cmd_ring: CommandRing,
    pub event_ring: EventRing,
    pub erst: DmaBuffer<[ErstEntry]>,
    pub pending_events: VecDeque<Trb>,
    pub max_ports: u8,
    pub phys_base: usize,
    context_size: usize,
}

impl HostController {
    pub fn init(regs: XhciRegisters, phys_base: usize) -> Result<Self, &'static str> {
        let hcs_params1 = regs.cap().hcs_params1.read();
        let hcc_params1 = regs.cap().hcc_params1.read();
        let max_ports = ((hcs_params1 >> 24) & 0xff) as u8;
        let max_slots = (hcs_params1 & 0xff) as u8;
        if max_ports == 0 || max_slots == 0 {
            return Err("xHCI reports no ports or device slots");
        }

        let context_size = if hcc_params1 & HCCPARAMS1_CSZ != 0 {
            64
        } else {
            32
        };
        let mut controller = Self {
            regs,
            dcbaa: DmaBuffer::new_slice(256, 0)?,
            cmd_ring: CommandRing::new(256)?,
            event_ring: EventRing::new(256)?,
            erst: DmaBuffer::new_slice(1, ErstEntry::default())?,
            pending_events: VecDeque::new(),
            max_ports,
            phys_base,
            context_size,
        };

        controller.reset()?;
        controller.configure(max_slots, hcc_params1 & HCCPARAMS1_PPC != 0)?;
        controller.start()?;

        Ok(controller)
    }

    fn reset(&mut self) -> Result<(), &'static str> {
        self.wait_for_bit(
            &self.regs.op().usb_sts,
            USBSTS_CNR,
            false,
            COMMAND_TIMEOUT_MS,
        )?;

        let command = self.regs.op().usb_cmd.read() & !USBCMD_RUN;
        self.regs.op().usb_cmd.write(command);
        self.wait_for_bit(
            &self.regs.op().usb_sts,
            USBSTS_HCH,
            true,
            COMMAND_TIMEOUT_MS,
        )?;

        self.regs.op().usb_cmd.write(USBCMD_HCRST);
        self.wait_for_bit(
            &self.regs.op().usb_cmd,
            USBCMD_HCRST,
            false,
            COMMAND_TIMEOUT_MS,
        )?;
        self.wait_for_bit(
            &self.regs.op().usb_sts,
            USBSTS_CNR,
            false,
            COMMAND_TIMEOUT_MS,
        )?;

        let page_sizes = self.regs.op().page_size.read();
        if page_sizes & 1 == 0 {
            return Err("xHCI does not support 4 KiB pages");
        }
        Ok(())
    }

    fn configure(&mut self, max_slots: u8, has_port_power: bool) -> Result<(), &'static str> {
        let op = self.regs.op();
        let mut config = op.config.read();
        config = (config & !0xff) | u32::from(max_slots);
        op.config.write(config);

        self.dcbaa.sync_for_device();
        op.dc_baap.write(self.dcbaa.phys_addr());
        op.crcr.write(self.cmd_ring.phys_addr() | 1);

        self.erst[0] = ErstEntry {
            seg_base: self.event_ring.phys_addr(),
            seg_size: self.event_ring.capacity() as u16,
            rsvd: [0; 3],
        };
        self.erst.sync_for_device();

        let interrupter = &self.regs.rt().irs[0];
        interrupter.er_stsz.write(1);
        interrupter.er_stba.write(self.erst.phys_addr());
        interrupter.erdp.write(self.event_ring.phys_addr());
        interrupter.imod.write(0);
        interrupter.iman.write(1);
        interrupter.iman.write(1 << 1);

        if has_port_power {
            self.power_on_ports()?;
        }
        delay_ms(100);

        Ok(())
    }

    fn power_on_ports(&mut self) -> Result<(), &'static str> {
        for port_id in 1..=self.max_ports {
            let portsc = self.portsc(port_id)?;
            let current = portsc.read();
            if current & PORTSC_PP == 0 {
                portsc.write((current & PORTSC_PIC_MASK) | PORTSC_PP);
            }
        }
        Ok(())
    }

    fn start(&mut self) -> Result<(), &'static str> {
        let command = self.regs.op().usb_cmd.read() | USBCMD_RUN | USBCMD_INTE;
        self.regs.op().usb_cmd.write(command);
        self.wait_for_bit(
            &self.regs.op().usb_sts,
            USBSTS_HCH,
            false,
            COMMAND_TIMEOUT_MS,
        )
    }

    fn wait_for_bit(
        &self,
        register: &Volatile<u32>,
        mask: u32,
        expected: bool,
        timeout_ms: u64,
    ) -> Result<(), &'static str> {
        let frequency = timer_frequency();
        let start = timer_counter();
        let timeout_ticks = frequency.saturating_mul(timeout_ms) / 1000;

        for _ in 0..FALLBACK_POLL_LIMIT {
            if (register.read() & mask != 0) == expected {
                return Ok(());
            }
            if frequency != 0 && timer_counter().wrapping_sub(start) >= timeout_ticks {
                return Err("xHCI register timeout");
            }
            core::hint::spin_loop();
        }

        Err("xHCI register timeout")
    }

    fn portsc(&self, port_id: u8) -> Result<&Volatile<u32>, &'static str> {
        if port_id == 0 || port_id > self.max_ports {
            return Err("xHCI root port is out of range");
        }

        let op = self.regs.op() as *const _ as *const u8;
        let offset = 0x400 + (usize::from(port_id) - 1) * 0x10;
        let register = unsafe { &*op.add(offset).cast::<Volatile<u32>>() };
        Ok(register)
    }

    fn reset_port(&self, port_id: u8) -> Result<Option<UsbSpeed>, &'static str> {
        let portsc = self.portsc(port_id)?;
        let status = portsc.read();
        if status & PORTSC_CCS == 0 {
            return Ok(None);
        }

        let speed_id = ((status & PORTSC_SPEED_MASK) >> PORTSC_SPEED_SHIFT) as u8;
        let speed = UsbSpeed::from_port_speed(speed_id).ok_or("unsupported USB port speed")?;
        let (reset_bit, reset_change_bit) =
            if matches!(speed, UsbSpeed::Super | UsbSpeed::SuperPlus) {
                (PORTSC_WPR, PORTSC_WRC)
            } else {
                (PORTSC_PR, PORTSC_PRC)
            };

        let previous_changes = status & PORTSC_CHANGE_MASK;
        if previous_changes != 0 {
            portsc.write((status & PORTSC_CONFIGURATION_MASK) | previous_changes);
        }
        portsc.write((status & PORTSC_CONFIGURATION_MASK) | reset_bit);
        let frequency = timer_frequency();
        let start = timer_counter();
        let timeout_ticks = frequency.saturating_mul(PORT_RESET_TIMEOUT_MS) / 1000;
        let mut reset_complete = false;

        for _ in 0..FALLBACK_POLL_LIMIT {
            let current = portsc.read();
            if current & PORTSC_CCS == 0 {
                return Ok(None);
            }
            if current & reset_bit == 0 && current & reset_change_bit != 0 {
                reset_complete = true;
                break;
            }
            if frequency != 0 && timer_counter().wrapping_sub(start) >= timeout_ticks {
                break;
            }
            core::hint::spin_loop();
        }

        if !reset_complete {
            return Err("xHCI root port reset timed out");
        }

        let status = portsc.read();
        if status & PORTSC_PED == 0 {
            return Err("xHCI root port did not enable after reset");
        }
        let changes = status & PORTSC_CHANGE_MASK;
        if changes != 0 {
            portsc.write((status & PORTSC_CONFIGURATION_MASK) | changes);
        }

        Ok(Some(speed))
    }

    pub fn enumerate_port(&mut self, port_id: u8) -> Result<Option<UsbDevice>, &'static str> {
        let Some(speed) = self.reset_port(port_id)? else {
            return Ok(None);
        };

        let enable_event = self.execute_command(Trb::command(TrbType::EnableSlotCommand))?;
        let slot_id = enable_event.slot_id();
        if slot_id == 0 || usize::from(slot_id) >= self.dcbaa.len() {
            return Err("xHCI returned an invalid slot ID");
        }

        let mut device = match UsbDevice::new(slot_id, port_id, speed, self.context_size) {
            Ok(device) => device,
            Err(error) => {
                let _ = self.disable_slot(slot_id);
                return Err(error);
            }
        };

        device.output_context.sync_for_device();
        self.dcbaa[usize::from(slot_id)] = device.output_context.phys_addr();
        self.dcbaa.sync_for_device();

        if let Err(error) = self.address_device(&mut device) {
            let _ = self.disable_slot(slot_id);
            return Err(error);
        }

        Ok(Some(device))
    }

    fn address_device(&mut self, device: &mut UsbDevice) -> Result<(), &'static str> {
        let stride = device.context_stride_words;
        device.input_context.fill(0);
        device.input_context[1] = 0b11;

        let slot_context = stride;
        device.input_context[slot_context] = (u32::from(device.speed.speed_id()) << 20) | (1 << 27);
        device.input_context[slot_context + 1] = u32::from(device.port_id) << 16;

        let ep0_context = stride * 2;
        device.input_context[ep0_context + 1] =
            (3 << 1) | (4 << 3) | (u32::from(device.ep0_max_packet_size) << 16);
        let dequeue = device.ep0_ring.dequeue_pointer();
        device.input_context[ep0_context + 2] = dequeue as u32;
        device.input_context[ep0_context + 3] = (dequeue >> 32) as u32;
        device.input_context[ep0_context + 4] = 8;
        device.input_context.sync_for_device();

        let command = Trb {
            parameter: device.input_context.phys_addr(),
            status: 0,
            control: ((TrbType::AddressDeviceCommand as u32) << 10)
                | (u32::from(device.slot_id) << 24),
        };
        self.execute_command(command)?;
        device.output_context.sync_for_cpu();
        Ok(())
    }

    pub fn evaluate_ep0_max_packet_size(
        &mut self,
        device: &mut UsbDevice,
        max_packet_size: u16,
    ) -> Result<(), &'static str> {
        let stride = device.context_stride_words;
        device.output_context.sync_for_cpu();
        device.input_context.fill(0);
        device.input_context[1] = 1 << 1;

        let output_ep0 = stride;
        let input_ep0 = stride * 2;
        device.input_context[input_ep0..input_ep0 + stride]
            .copy_from_slice(&device.output_context[output_ep0..output_ep0 + stride]);
        let old = device.input_context[input_ep0 + 1];
        device.input_context[input_ep0 + 1] =
            (old & !(0xffff << 16)) | (u32::from(max_packet_size) << 16);
        device.input_context.sync_for_device();

        let command = Trb {
            parameter: device.input_context.phys_addr(),
            status: 0,
            control: ((TrbType::EvaluateContextCommand as u32) << 10)
                | (u32::from(device.slot_id) << 24),
        };
        self.execute_command(command)?;
        device.output_context.sync_for_cpu();
        device.ep0_max_packet_size = max_packet_size;
        Ok(())
    }

    pub fn configure_bulk_endpoints(&mut self, device: &mut UsbDevice) -> Result<(), &'static str> {
        let bulk_in = device
            .bulk_in
            .as_ref()
            .ok_or("USB mass-storage IN endpoint is missing")?;
        let bulk_out = device
            .bulk_out
            .as_ref()
            .ok_or("USB mass-storage OUT endpoint is missing")?;
        let input_dci = bulk_in.dci;
        let output_dci = bulk_out.dci;
        let highest_dci = input_dci.max(output_dci);
        let add_context_flags =
            (1u32 << 0) | (1u32 << 1) | (1u32 << input_dci) | (1u32 << output_dci);
        let stride = device.context_stride_words;

        device.output_context.sync_for_cpu();
        device.input_context.fill(0);
        device.input_context[1] = add_context_flags;

        device.input_context[stride..stride * 2].copy_from_slice(&device.output_context[..stride]);
        let slot_dword0 = stride;
        device.input_context[slot_dword0] =
            (device.input_context[slot_dword0] & !(0x1f << 27)) | (u32::from(highest_dci) << 27);

        let output_ep0 = stride;
        let input_ep0 = stride * 2;
        device.input_context[input_ep0..input_ep0 + stride]
            .copy_from_slice(&device.output_context[output_ep0..output_ep0 + stride]);

        write_endpoint_context(&mut device.input_context, stride, bulk_in, 6);
        write_endpoint_context(&mut device.input_context, stride, bulk_out, 2);
        device.input_context.sync_for_device();

        let command = Trb {
            parameter: device.input_context.phys_addr(),
            status: 0,
            control: ((TrbType::ConfigureEndpointCommand as u32) << 10)
                | (u32::from(device.slot_id) << 24),
        };
        self.execute_command(command)?;
        device.output_context.sync_for_cpu();
        Ok(())
    }

    fn execute_command(&mut self, command: Trb) -> Result<Trb, &'static str> {
        let command_phys = self.cmd_ring.enqueue(command)?;
        self.regs.ring_doorbell(0, 0);
        let event = self.wait_for_event(
            |event| {
                event.trb_type() == Some(TrbType::CommandCompletionEvent)
                    && event.parameter & !0xf == command_phys & !0xf
            },
            COMMAND_TIMEOUT_MS,
        )?;
        if event.completion_code() != 1 {
            log::error!(
                "xhci: command failed (completion code {}, slot {})",
                event.completion_code(),
                event.slot_id()
            );
            return Err("xHCI command failed");
        }
        Ok(event)
    }

    fn wait_for_event(
        &mut self,
        mut matches: impl FnMut(&Trb) -> bool,
        timeout_ms: u64,
    ) -> Result<Trb, &'static str> {
        let frequency = timer_frequency();
        let start = timer_counter();
        let timeout_ticks = frequency.saturating_mul(timeout_ms) / 1000;

        for _ in 0..FALLBACK_POLL_LIMIT {
            self.process_events();
            if let Some(index) = self.pending_events.iter().position(&mut matches) {
                return self
                    .pending_events
                    .remove(index)
                    .ok_or("xHCI event disappeared from queue");
            }
            if frequency != 0 && timer_counter().wrapping_sub(start) >= timeout_ticks {
                return Err("xHCI event timed out");
            }
            core::hint::spin_loop();
        }

        Err("xHCI event timed out")
    }

    fn wait_for_transfer(
        &mut self,
        slot_id: u8,
        endpoint_id: u8,
        trb_phys: u64,
        requested_length: usize,
    ) -> Result<usize, &'static str> {
        let event = self.wait_for_event(
            |event| {
                event.trb_type() == Some(TrbType::TransferEvent)
                    && event.slot_id() == slot_id
                    && event.endpoint_id() == endpoint_id
                    && event.parameter & !0xf == trb_phys & !0xf
            },
            TRANSFER_TIMEOUT_MS,
        )?;

        match event.completion_code() {
            1 | 13 => {
                let residual = (event.status & 0x00ff_ffff) as usize;
                requested_length
                    .checked_sub(residual)
                    .ok_or("xHCI reported an invalid transfer length")
            }
            code => {
                log::warn!(
                    "xhci: transfer failed (completion code {}, slot {}, endpoint {})",
                    code,
                    slot_id,
                    endpoint_id
                );
                Err("xHCI USB transfer failed")
            }
        }
    }

    pub fn control_transfer(
        &mut self,
        device: &mut UsbDevice,
        setup: SetupPacket,
        data: &mut [u8],
    ) -> Result<usize, &'static str> {
        let length = usize::from(setup.length);
        if length > MAX_TRANSFER_SIZE || data.len() < length {
            return Err("USB control transfer buffer is too small");
        }

        if length != 0 && !setup.direction_in() {
            device.transfer_buffer[..length].copy_from_slice(&data[..length]);
        }
        if length != 0 {
            device.transfer_buffer.sync_for_device();
        }

        device.ep0_ring.enqueue(Trb::setup_stage(
            setup.parameter(),
            setup.transfer_type(),
            true,
        ))?;
        if length != 0 {
            device.ep0_ring.enqueue(Trb::data_stage(
                device.transfer_buffer.phys_addr(),
                length as u32,
                setup.direction_in(),
                true,
            ))?;
        }
        let status_pointer = device
            .ep0_ring
            .enqueue(Trb::status_stage(length == 0 || !setup.direction_in()))?;
        self.regs.ring_doorbell(device.slot_id, 1);

        let event = self.wait_for_event(
            |event| {
                event.trb_type() == Some(TrbType::TransferEvent)
                    && event.slot_id() == device.slot_id
                    && event.endpoint_id() == 1
                    && event.parameter & !0xf == status_pointer & !0xf
            },
            TRANSFER_TIMEOUT_MS,
        )?;
        if !matches!(event.completion_code(), 1 | 13) {
            return Err("USB control transfer failed");
        }

        let residual = (event.status & 0x00ff_ffff) as usize;
        let transferred = length
            .checked_sub(residual)
            .ok_or("xHCI reported an invalid control transfer length")?;
        if setup.direction_in() && transferred != 0 {
            device.transfer_buffer.sync_for_cpu();
            data[..transferred].copy_from_slice(&device.transfer_buffer[..transferred]);
        }
        Ok(transferred)
    }

    pub fn bulk_out(
        &mut self,
        device: &mut UsbDevice,
        endpoint_address: u8,
        data: &[u8],
    ) -> Result<(), &'static str> {
        if endpoint_address & 0x80 != 0 {
            return Err("USB bulk OUT endpoint has an IN address");
        }
        let slot_id = device.slot_id;
        let endpoint = device
            .bulk_out
            .as_mut()
            .filter(|endpoint| endpoint.address == endpoint_address)
            .ok_or("USB bulk OUT endpoint is not configured")?;

        for chunk in data.chunks(MAX_TRANSFER_SIZE) {
            device.transfer_buffer[..chunk.len()].copy_from_slice(chunk);
            device.transfer_buffer.sync_for_device();
            let trb_phys = endpoint.ring.enqueue(Trb::normal(
                device.transfer_buffer.phys_addr(),
                chunk.len() as u32,
                false,
            ))?;
            self.regs.ring_doorbell(slot_id, endpoint.dci);
            let transferred =
                self.wait_for_transfer(slot_id, endpoint.dci, trb_phys, chunk.len())?;
            if transferred != chunk.len() {
                return Err("short USB bulk OUT transfer");
            }
        }

        Ok(())
    }

    pub fn bulk_in(
        &mut self,
        device: &mut UsbDevice,
        endpoint_address: u8,
        data: &mut [u8],
    ) -> Result<usize, &'static str> {
        if endpoint_address & 0x80 == 0 {
            return Err("USB bulk IN endpoint has an OUT address");
        }
        let slot_id = device.slot_id;
        let endpoint = device
            .bulk_in
            .as_mut()
            .filter(|endpoint| endpoint.address == endpoint_address)
            .ok_or("USB bulk IN endpoint is not configured")?;

        let mut total_transferred = 0;
        for chunk in data.chunks_mut(MAX_TRANSFER_SIZE) {
            device.transfer_buffer.sync_for_device();
            let trb_phys = endpoint.ring.enqueue(Trb::normal(
                device.transfer_buffer.phys_addr(),
                chunk.len() as u32,
                true,
            ))?;
            self.regs.ring_doorbell(slot_id, endpoint.dci);
            let transferred =
                self.wait_for_transfer(slot_id, endpoint.dci, trb_phys, chunk.len())?;
            device.transfer_buffer.sync_for_cpu();
            chunk[..transferred].copy_from_slice(&device.transfer_buffer[..transferred]);
            total_transferred += transferred;
            if transferred != chunk.len() {
                return Ok(total_transferred);
            }
        }

        Ok(total_transferred)
    }

    pub fn disable_slot(&mut self, slot_id: u8) -> Result<(), &'static str> {
        self.execute_command(Trb::command_with_slot(TrbType::DisableSlotCommand, slot_id))?;
        if usize::from(slot_id) < self.dcbaa.len() {
            self.dcbaa[usize::from(slot_id)] = 0;
            self.dcbaa.sync_for_device();
        }
        Ok(())
    }

    pub fn process_events(&mut self) {
        let status = self.regs.op().usb_sts.read();
        let status_to_clear = status & (USBSTS_HSE | USBSTS_EINT | USBSTS_PCD);
        if status_to_clear != 0 {
            self.regs.op().usb_sts.write(status_to_clear);
        }

        let iman = self.regs.rt().irs[0].iman.read();
        if iman & 1 != 0 {
            self.regs.rt().irs[0].iman.write((iman & (1 << 1)) | 1);
        }

        let mut count = 0;
        while let Some(event) = self.event_ring.next_event() {
            count += 1;
            self.handle_event(event);
        }

        if count != 0 {
            self.regs.rt().irs[0]
                .erdp
                .write(self.event_ring.current_erdp() | (1 << 3));
        }
    }

    fn handle_event(&mut self, event: Trb) {
        match event.trb_type() {
            Some(TrbType::CommandCompletionEvent)
            | Some(TrbType::PortStatusChangeEvent)
            | Some(TrbType::TransferEvent) => self.pending_events.push_back(event),
            other => log::debug!(
                "xhci: unhandled event TRB {:?} (raw type={})",
                other,
                event.raw_trb_type()
            ),
        }
    }
}

fn write_endpoint_context(
    input_context: &mut [u32],
    stride: usize,
    endpoint: &UsbEndpoint,
    endpoint_type: u32,
) {
    let start = (usize::from(endpoint.dci) + 1) * stride;
    let dequeue = endpoint.ring.dequeue_pointer();
    input_context[start] = 0;
    input_context[start + 1] =
        (3 << 1) | (endpoint_type << 3) | (u32::from(endpoint.max_packet_size) << 16);
    input_context[start + 2] = dequeue as u32;
    input_context[start + 3] = (dequeue >> 32) as u32;
    input_context[start + 4] = u32::from(endpoint.max_packet_size);
}

fn timer_counter() -> u64 {
    let value;
    unsafe {
        asm!(
            "mrs {value}, cntpct_el0",
            value = out(reg) value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

fn timer_frequency() -> u64 {
    let value;
    unsafe {
        asm!(
            "mrs {value}, cntfrq_el0",
            value = out(reg) value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

fn delay_ms(milliseconds: u64) {
    let frequency = timer_frequency();
    if frequency == 0 {
        for _ in 0..(FALLBACK_POLL_LIMIT / 100) {
            core::hint::spin_loop();
        }
        return;
    }

    let start = timer_counter();
    let ticks = frequency.saturating_mul(milliseconds) / 1000;
    while timer_counter().wrapping_sub(start) < ticks {
        core::hint::spin_loop();
    }
}
