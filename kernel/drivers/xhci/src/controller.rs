use aarch64_cpu_ext::asm::sev;
use alloc::collections::vec_deque::VecDeque;

use crate::{
    dma::DmaBuffer,
    registers::{ErstEntry, Volatile, XhciRegisters},
    ring::{CommandRing, EventRing},
    trb::{Trb, TrbType},
};

const USBCMD_RUN: u32 = 1 << 0;
const USBCMD_HCRST: u32 = 1 << 1;
const USBCMD_INTE: u32 = 1 << 2;
const USBSTS_HCH: u32 = 1 << 0;
const USBSTS_CNR: u32 = 1 << 11;

// write 1 to clear
const PORTSC_RW1C_MASK: u32 = 0x7F << 17;
const PORTSC_PP: u32 = 1 << 9; // port... with power

pub struct HostController {
    pub regs: XhciRegisters,
    pub dcbaa: DmaBuffer<[u64]>,
    pub cmd_ring: CommandRing,
    pub event_ring: EventRing,
    pub erst: DmaBuffer<[ErstEntry]>,
    pub pending_events: VecDeque<Trb>,
}

impl HostController {
    pub fn init(regs: XhciRegisters) -> Result<Self, &'static str> {
        let mut controller = Self {
            regs,
            dcbaa: DmaBuffer::new_slice(256, 0)?,
            cmd_ring: CommandRing::new(64)?,
            event_ring: EventRing::new(256)?,
            erst: DmaBuffer::new_slice(1, ErstEntry::default())?,
            pending_events: VecDeque::new(),
        };

        controller.reset()?;
        controller.configure()?;
        controller.start()?;

        Ok(controller)
    }

    fn wait_for_bit(
        &self,
        reg: &Volatile<u32>,
        bit: u32,
        expected: bool,
    ) -> Result<(), &'static str> {
        let limit = 100_000;
        for _ in 0..limit {
            let val = reg.read();
            if ((val & bit) != 0) == expected {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err("h/w timeout")
    }

    fn reset(&self) -> Result<(), &'static str> {
        // must wait until CNR = 0 before writing op regs
        self.wait_for_bit(&self.regs.op().usb_sts, USBSTS_CNR, false)?;

        let mut cmd = self.regs.op().usb_cmd.read();
        cmd &= !USBCMD_RUN;
        self.regs.op().usb_cmd.write(cmd);
        self.wait_for_bit(&self.regs.op().usb_sts, USBSTS_HCH, true)?;

        self.regs.op().usb_cmd.write(USBCMD_HCRST);
        self.wait_for_bit(&self.regs.op().usb_cmd, USBCMD_HCRST, false)?;
        self.wait_for_bit(&self.regs.op().usb_sts, USBSTS_CNR, false)?;

        Ok(())
    }

    fn configure(&mut self) -> Result<(), &'static str> {
        let op = self.regs.op();
        let hcs_params1 = self.regs.cap().hcs_params1.read();
        let max_slots = (hcs_params1 & 0xFF) as u8;
        let max_ports = ((hcs_params1 >> 24) & 0xFF) as usize;

        // set max device slots enabled.
        let mut config = op.config.read();
        config = (config & !0xFF) | (max_slots as u32);
        op.config.write(config);

        // device context base addr array
        op.dc_baap.write(self.dcbaa.phys_addr());

        // cmd ring: [0] is Ring Cycle State (RCS = 1)
        op.crcr.write(self.cmd_ring.phys_addr() | 1);

        // config event ring segment table
        let erstba = self.erst.phys_addr();
        let ers_size = self.event_ring.capacity();

        self.erst[0] = ErstEntry {
            seg_base: self.event_ring.phys_addr(),
            seg_size: ers_size as u16,
            rsvd: [0; 3],
        };

        let rt = self.regs.rt();
        rt.irs[0].er_stsz.write(1);
        rt.irs[0].er_stba.write(erstba);
        rt.irs[0].erdp.write(self.event_ring.phys_addr());

        // fire interrupts immediately
        rt.irs[0].imod.write(0);

        // enable IMAN interrupter. IE [0] and IP [1]
        rt.irs[0].iman.write(3);

        self.power_on_ports(max_ports);

        Ok(())
    }

    fn power_on_ports(&mut self, max_ports: usize) {
        let op_ptr = self.regs.op() as *const _ as *mut u8;
        for port in 0..max_ports {
            let portsc_ptr = unsafe {
                (op_ptr.add(0x400 + port * 0x10))
                    .cast::<Volatile<u32>>()
                    .as_ref()
            };
            if let Some(portsc) = portsc_ptr {
                let current = portsc.read();
                let new_val = (current & !PORTSC_RW1C_MASK) | PORTSC_PP;
                portsc.write(new_val);
            }
        }
    }

    fn start(&mut self) -> Result<(), &'static str> {
        let cmd = self.regs.op().usb_cmd.read() | USBCMD_RUN | USBCMD_INTE;
        self.regs.op().usb_cmd.write(cmd);

        // wait for hardware to parse schedules
        self.wait_for_bit(&self.regs.op().usb_sts, USBSTS_HCH, false)?;

        Ok(())
    }

    pub fn test_interrupt(&mut self) -> Result<(), &'static str> {
        self.send_command(Trb::command(TrbType::NoOpCommand))
    }

    pub fn send_command(&mut self, trb: Trb) -> Result<(), &'static str> {
        self.cmd_ring.enqueue(trb)?;
        self.regs.ring_doorbell(0, 0); // gem alarm... doorbell 0 is the host controller cmd ring
        Ok(())
    }

    pub fn process_events(&mut self) {
        let iman = self.regs.rt().irs[0].iman.read();
        if (iman & 1) != 0 {
            // clear IP [0] by writing 1 (RW1C)
            self.regs.rt().irs[0].iman.write(iman | 1);
        }

        let mut count = 0;
        while let Some(event) = self.event_ring.next_event() {
            count += 1;
            self.handle_event(event);
        }

        if count > 0 {
            // ack hardware by advancing th dequeue ptr and clearing EHB [3]
            let erdp = self.event_ring.current_erdp();
            self.regs.rt().irs[0].erdp.write(erdp | (1 << 3));
        }
    }

    fn handle_event(&mut self, trb: Trb) {
        use log::*;

        match trb.trb_type() {
            Some(TrbType::CommandCompletionEvent)
            | Some(TrbType::PortStatusChangeEvent)
            | Some(TrbType::TransferEvent) => {
                self.pending_events.push_back(trb);
                sev();
            }
            other => {
                debug!(
                    "xhci: unhandled event TRB {:?} (raw type={})",
                    other,
                    trb.raw_trb_type()
                );
            }
        }
    }

    pub fn pop_event(&mut self) -> Option<Trb> {
        self.pending_events.pop_front()
    }
}
