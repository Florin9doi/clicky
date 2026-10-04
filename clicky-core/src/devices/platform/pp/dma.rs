use crate::devices::prelude::*;

use std::time::{Duration, Instant};

const CMD_SIZE_MASK: u32 = 0xffff;
const CMD_REQ_ID_SHIFT: u32 = 16;
const CMD_REQ_ID_MASK: u32 = 0xf << CMD_REQ_ID_SHIFT;
const CMD_WAIT_REQ: u32 = 1 << 24;
const CMD_SINGLE: u32 = 1 << 26;
const CMD_RAM_TO_PER: u32 = 1 << 27;
const CMD_SLEEP_WAIT: u32 = 1 << 28;
const CMD_INTR: u32 = 1 << 30;
const CMD_START: u32 = 1 << 31;

const STATUS_SIZE_REMAIN_MASK: u32 = 0xffff;
const STATUS_INTR: u32 = 1 << 30;
const STATUS_BUSY: u32 = 1 << 31;

const INCR_RANGE_MASK: u32 = 0x7 << 16;
const INCR_RANGE_UNL: u32 = 0x0 << 16;
const INCR_RANGE_FIXED: u32 = 0x1 << 16;
const INCR_RANGE_ALTR: u32 = 0x2 << 16;
const INCR_RANGE_4: u32 = 0x3 << 16;
const INCR_RANGE_8: u32 = 0x4 << 16;
const INCR_RANGE_16: u32 = 0x5 << 16;
const INCR_RANGE_32: u32 = 0x6 << 16;
const INCR_RANGE_64: u32 = 0x7 << 16;
const INCR_WIDTH_MASK: u32 = 0x7 << 28;
const INCR_WIDTH_8BIT: u32 = 0x0 << 28;
const INCR_WIDTH_16BIT: u32 = 0x1 << 28;
const INCR_WIDTH_32BIT: u32 = 0x2 << 28;

const MASTER_CONTROL_EN: u32 = 1 << 31;
const MASTER_STATUS_CH_SHIFT: u32 = 24; // CH0 at bit24, CH1 at 25, CH2 at 26, CH3 at 27

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaWidth {
    Byte,
    Half,
    Word,
}
impl DmaWidth {
    fn bytes(self) -> u32 {
        match self {
            DmaWidth::Byte => 1,
            DmaWidth::Half => 2,
            DmaWidth::Word => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaEngine {
    Con0,
    Con1,
}

#[derive(Debug, Clone, Copy)]
pub struct DmaXfer {
    pub channel: usize,
    pub width: DmaWidth,
    pub count: u32,
    pub src: u32,
    pub src_step: u32,
    pub dst: u32,
    pub dst_step: u32,
    pub want_intr: bool,
}

#[derive(Debug, Default)]
struct Dma {
    label: Option<&'static str>,

    cmd: u32,
    status: u32,
    ram_addr: u32,
    flags: u32,
    per_addr: u32,
    incr: u32,
}

impl Dma {
    fn decode_xfer(&self, channel: usize) -> DmaXfer {
        let width = match self.incr & INCR_WIDTH_MASK {
            INCR_WIDTH_8BIT => DmaWidth::Byte,
            INCR_WIDTH_16BIT => DmaWidth::Half,
            INCR_WIDTH_32BIT => DmaWidth::Word,
            _ => DmaWidth::Word,
        };
        let width_bytes = width.bytes();
        let count = (self.cmd & CMD_SIZE_MASK) + 4;

        let per_step = match self.incr & INCR_RANGE_MASK {
            INCR_RANGE_FIXED => 0,
            INCR_RANGE_4 => 4,
            INCR_RANGE_8 => 8,
            INCR_RANGE_16 => 16,
            INCR_RANGE_32 => 32,
            INCR_RANGE_64 => 64,
            _ => width_bytes,
        };

        DmaXfer {
            channel,
            width,
            count,
            src: if self.cmd & CMD_RAM_TO_PER != 0 {self.ram_addr} else {self.per_addr},
            src_step: width_bytes,
            dst: if self.cmd & CMD_RAM_TO_PER != 0 {self.per_addr} else {self.ram_addr},
            dst_step: per_step,
            want_intr: self.cmd & CMD_INTR != 0,
        }
    }
}

impl Device for Dma {
    fn kind(&self) -> &'static str {
        "<dma>"
    }

    fn label(&self) -> Option<&'static str> {
        self.label
    }

    fn probe(&self, offset: u32) -> Probe {
        let reg = match offset {
            0x00 => "Cmd",
            0x04 => "Status",
            0x10 => "Ram Addr",
            0x14 => "Flags",
            0x18 => "Per Addr",
            0x1c => "Incr",
            _ => return Probe::Unmapped,
        };

        Probe::Register(reg)
    }
}

#[derive(Debug)]
pub struct DmaCon {
    label: &'static str,

    dma: [Dma; 8],
    master_control: u32,
    master_status: u32,
    req_status: u32,
    ready_mask: u8,
    irq: Option<irq::Sender>,

    // HACK: IDE DMA doesn't actually go through the DMA controller
    // that said, to keep things simple in the emulator, we route IDE DMA through the main DMA
    // engine...
    //
    // As per the pp5020 spec sheet: "A dedicated, high-performance ATA-66IDE controller with its
    // own DMA engine frees the processors from mundane management tasks."
    //
    // Only the engine the IDE controller was wired to carries this; the other
    // one gets `None`.
    ide_dmarq: Option<irq::Receiver>,
    last_run: Instant,
}

impl DmaCon {
    pub fn new(
        label: &'static str,
        ide_dmarq: Option<irq::Receiver>,
        irq: Option<irq::Sender>,
    ) -> DmaCon {
        let mut dma = DmaCon {
            label,

            dma: Default::default(),
            master_control: 0,
            master_status: 0,
            req_status: 0,

            ready_mask: 0,
            irq,

            ide_dmarq,
            last_run: Instant::now(),
        };

        dma.dma[0].label = Some("0");
        dma.dma[1].label = Some("1");
        dma.dma[2].label = Some("2");
        dma.dma[3].label = Some("3");
        dma.dma[4].label = Some("4");
        dma.dma[5].label = Some("5");
        dma.dma[6].label = Some("6");
        dma.dma[7].label = Some("7");

        dma
    }

    /// XXX: remove this once DMA is properly sorted out
    pub fn do_ide_dma(&self) -> bool {
        if let Some(ref dmarq) = self.ide_dmarq {
            dmarq.asserted()
        } else {
            false
        }
    }

    pub fn take_ready(&mut self) -> Option<DmaXfer> {
        if self.ready_mask == 0 {
            return None;
        }

        // TODO: it's the hack time
        // if self.last_run.elapsed() < Duration::from_millis(100) {
        //     return None;
        // }
        // self.last_run = Instant::now();

        let channel = self.ready_mask.trailing_zeros() as usize;
        if self.dma[channel].ram_addr == 0 || self.dma[channel].per_addr == 0 || self.dma[channel].cmd & CMD_SIZE_MASK == 0 {
            return None;
        }
        self.ready_mask &= !(1 << channel);

        Some(self.dma[channel].decode_xfer(channel))
    }

    pub fn complete(&mut self, channel: usize, want_intr: bool) {
        let dma = &mut self.dma[channel];
        dma.cmd &= !CMD_START;
        dma.status &= !STATUS_BUSY;
        dma.status &= !STATUS_SIZE_REMAIN_MASK;

        if want_intr {
            dma.status |= STATUS_INTR;
            if let Some(irq) = &mut self.irq {
                irq.assert();
            }
        }
    }
}

impl Device for DmaCon {
    fn kind(&self) -> &'static str {
        "DMA Engine"
    }

    fn label(&self) -> Option<&'static str> {
        Some(self.label)
    }

    fn probe(&self, offset: u32) -> Probe {
        let reg = match offset {
            0x0 => "Master Control",
            0x4 => "Master Status",
            0x8 => "Req Status",
            0x1000..=0x10ff => {
                let id = (offset - 0x1000) / 0x20;
                return Probe::from_device(&self.dma[id as usize], offset % 0x20);
            }
            _ => return Probe::Unmapped,
        };

        Probe::Register(reg)
    }
}

impl Memory for DmaCon {
    fn r32(&mut self, offset: u32) -> MemResult<u32> {
        match offset {
            0x0 => Err(StubRead(Error, self.master_control)),
            0x4 => {
                let mut status = 0;
                for (i, dma) in self.dma.iter().enumerate().take(4) {
                    if dma.status & STATUS_INTR != 0 {
                        status |= 1 << (MASTER_STATUS_CH_SHIFT + i as u32);
                    }
                }
                Err(StubRead(Error, status))
            }
            0x4 => Err(StubRead(Error, self.master_status)),
            0x8 => Err(StubRead(Error, self.req_status)),
            0x1000..=0x10ff => {
                let id = (offset - 0x1000) / 0x20;
                let dma = &mut self.dma[id as usize];
                match offset % 0x20 {
                    0x00 => Err(StubRead(Error, dma.cmd)),
                    0x04 => Err(StubRead(Error, dma.status)),
                    0x10 => Err(StubRead(Error, dma.ram_addr)),
                    0x14 => Err(StubRead(Error, dma.flags)),
                    0x18 => Err(StubRead(Error, dma.per_addr)),
                    0x1c => Err(StubRead(Error, dma.incr)),
                    _ => Err(Unexpected),
                }
            }
            _ => Err(Unexpected),
        }
    }

    fn w32(&mut self, offset: u32, val: u32) -> MemResult<()> {
        match offset {
            0x0 => Err(StubWrite(Error, self.master_control = val)),
            // 0x4 => Err(StubWrite(Error, self.master_status = val)),
            0x4 => Err(Unexpected),
            0x8 => Err(StubWrite(Error, self.req_status = val)),
            0x1000..=0x10ff => {
                let id = (offset - 0x1000) / 0x20;
                let channel = id as usize;
                let dma = &mut self.dma[channel];
                match offset % 0x20 {
                    0x00 => {
                        dma.cmd = val;
                        if val & CMD_WAIT_REQ != 0 {
                        // if val & CMD_START != 0 {
                            if dma.status & STATUS_BUSY != 0 {
                                // already running
                            } else {
                                dma.status |= STATUS_BUSY;
                                dma.status &= !STATUS_INTR;
                                self.ready_mask |= 1 << channel;
                            }
                        }
                        Ok(())
                    }
                    0x04 => {
                        // dma.status &= !val;
                        // Ok(())
                        Err(Unexpected)
                    }
                    0x10 => Err(StubWrite(Error, dma.ram_addr = val)),
                    0x14 => Err(StubWrite(Error, dma.flags = val)),
                    0x18 => Err(StubWrite(Error, dma.per_addr = val)),
                    0x1c => Err(StubWrite(Error, dma.incr = val)),
                    _ => Err(Unexpected),
                }
            }
            _ => Err(Unexpected),
        }
    }
}
