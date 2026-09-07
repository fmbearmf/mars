

use crate::{
    dma::DmaBuffer,
    registers::{ErstEntry, Volatile, XhciRegisters},
    ring::{CommandRing, EventRing},
    trb::{Trb, TrbType},
};

const USBCMD_RUN: u32 = 1 << 0;
const USBCMD_HCRST: u32 = 1 << 1;
const USBSTS_HCH: u32 = 1 << 0;
const USBSTS_CNR: u32 = 1 << 11;

pub struct HostController {
    regs: XhciRegisters,
    dcbaa: DmaBuffer<[u64]>,
    cmd_ring: CommandRing,
    event_ring: EventRing,
    erst: DmaBuffer<[ErstEntry]>,
}

impl HostController {
    pub fn init(regs: XhciRegisters) -> Result<Self, &'static str> {
        let mut controller = Self {
            regs,
            dcbaa: DmaBuffer::new_slice(256, 0)?,
            cmd_ring: CommandRing::new(64)?,
            event_ring: EventRing::new(256)?,
            erst: DmaBuffer::new_slice(1, ErstEntry::default())?,
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

        // enable IMAN interrupter. IE [0] and IP [1]
        rt.irs[0].iman.write(3);

        Ok(())
    }

    fn start(&mut self) -> Result<(), &'static str> {
        let cmd = self.regs.op().usb_cmd.read() | USBCMD_RUN;
        self.regs.op().usb_cmd.write(cmd);

        // wait for hardware to parse schedules
        self.wait_for_bit(&self.regs.op().usb_sts, USBSTS_HCH, false)?;

        // drain any
        self.process_events();
        Ok(())
    }

    pub fn send_command(&mut self, trb: Trb) -> Result<(), &'static str> {
        self.cmd_ring.enqueue(trb)?;
        self.regs.ring_doorbell(0, 0); // gem alarm...
        Ok(())
    }

    pub fn process_events(&mut self) {
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
            Some(TrbType::CommandCompletionEvent) => {
                info!(
                    "xhci: command completed: code={}, slot_id={}",
                    trb.completion_code(),
                    trb.slot_id()
                );
            }
            Some(TrbType::PortStatusChangeEvent) => {
                let port_id = (trb.parameter >> 24) as u8;
                info!("xhci: port status changed on port {}", port_id);
            }
            Some(TrbType::TransferEvent) => {
                debug!("xhci: transfer event: {:?}", trb);
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
}
