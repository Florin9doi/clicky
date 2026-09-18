use crate::devices::prelude::*;

use std::collections::HashMap;

const DATA: u32 = 0x00000;
const WRITE_ADDR: u32 = 0x10000;
const READ_ADDR: u32 = 0x20000;
const STATUS: u32 = 0x30000;
const READY: u32 = 0x60000;
const HANDSHAKE: u32 = 0x70000;

const VALIDATION_REG: u32 = 0x10000c00;
const DOORBELL_REG: u32 = 0x000001f8;
const DOORBELL_DONE: u32 = 1;

#[derive(Debug)]
pub struct Bcm2722 {
    label: &'static str,

    // Indirect write-address latch.
    waddr: u32,
    wph: bool,

    // Indirect read-address latch.
    raddr: u32,
    rph: bool,

    // Data-port write state.
    dwval: u32,
    dwph: bool,

    // Data-port read state.
    drval: u32,
    drph: bool,

    // BCM internal register file.
    regs: HashMap<u32, u32>,

    // Useful for debugging / validation.
    regwrites: u64,
    regreads: u64,
    fbwrites: u64,

    passed_validation: bool,
    doorbell_acks: u64,
}

impl Bcm2722 {
    pub fn new() -> Bcm2722 {
        Bcm2722 {
            label: "BCM2722",

            waddr: 0,
            wph: false,

            raddr: 0,
            rph: false,

            dwval: 0,
            dwph: false,

            drval: 0,
            drph: false,

            regs: HashMap::new(),

            regwrites: 0,
            regreads: 0,
            fbwrites: 0,

            passed_validation: false,
            doorbell_acks: 0,
        }
    }

    fn read_data(&mut self) -> u16 {
        if !self.drph {
            self.regreads += 1;

            self.drval = match self.raddr {
                // osos validation gate.
                VALIDATION_REG => {
                    self.passed_validation = true;
                    0x0000_0001
                }

                // Post-frame completion doorbell.
                DOORBELL_REG => {
                    self.doorbell_acks += 1;
                    DOORBELL_DONE
                }

                // Ordinary BCM register.
                addr => *self.regs.get(&addr).unwrap_or(&0),
            };

            self.drph = true;

            (self.drval & 0xffff) as u16
        } else {
            self.drph = false;

            ((self.drval >> 16) & 0xffff) as u16
        }
    }

    fn write_data(&mut self, val: u16) {
        self.fbwrites += 1;

        if !self.dwph {
            // First half = low 16 bits.
            self.dwval = val as u32;
            self.dwph = true;
        } else {
            // Second half = high 16 bits.
            self.dwval |= (val as u32) << 16;
            self.dwph = false;

            self.regs.insert(self.waddr, self.dwval);
            self.regwrites += 1;
        }
    }

    fn write_address(&mut self, val: u16) {
        if !self.wph {
            self.waddr = val as u32;
            self.wph = true;
        } else {
            self.waddr |= (val as u32) << 16;
            self.wph = false;
        }
    }

    fn read_address(&mut self, val: u16) {
        if !self.rph {
            self.raddr = val as u32;
            self.rph = true;
        } else {
            self.raddr |= (val as u32) << 16;
            self.rph = false;

            // A new address selection starts a new 32-bit read.
            self.drph = false;
        }
    }

    pub fn passed_validation(&self) -> bool {
        self.passed_validation
    }

    pub fn regreads(&self) -> u64 {
        self.regreads
    }

    pub fn regwrites(&self) -> u64 {
        self.regwrites
    }

    pub fn fbwrites(&self) -> u64 {
        self.fbwrites
    }

    pub fn doorbell_acks(&self) -> u64 {
        self.doorbell_acks
    }

    pub fn register(&self, addr: u32) -> u32 {
        *self.regs.get(&addr).unwrap_or(&0)
    }
}

impl Device for Bcm2722 {
    fn kind(&self) -> &'static str {
        "BCM2722"
    }

    fn label(&self) -> Option<&'static str> {
        Some(self.label)
    }

    fn probe(&self, offset: u32) -> Probe {
        let reg = match offset {
            DATA => "DATA",
            WRITE_ADDR => "WRITE_ADDR",
            READ_ADDR => "READ_ADDR",
            STATUS => "STATUS",
            READY => "READY",
            HANDSHAKE => "HANDSHAKE",
            _ => return Probe::Unmapped,
        };
        Probe::Register(reg)
    }
}

impl Memory for Bcm2722 {
    fn r8(&mut self, offset: u32) -> MemResult<u8> {
        match offset {
            _ => Err(StubRead(Error, 0)),
        }
    }

    fn r16(&mut self, offset: u32) -> MemResult<u16> {
        match offset {
            STATUS => Err(StubRead(Trace, 0x0013)),
            0x20000 | 0x60000 | 0x10000 | 0x50000 | 0x40000 => {
                Err(StubRead(Trace, 0x0001))
            }
            HANDSHAKE => Err(StubRead(Trace, 0x0040)),
            DATA => Err(StubRead(Trace, self.read_data() as u32)),
            _ => Err(StubRead(Error, 0)),
        }
    }

    fn r32(&mut self, offset: u32) -> MemResult<u32> {
        match offset {
            DATA => {
                let lo = self.read_data() as u32;
                let hi = self.read_data() as u32;

                Err(StubRead(Trace, lo | (hi << 16)))
            }
            STATUS => Err(StubRead(Trace, 0x13)),
            HANDSHAKE => Err(StubRead(Trace, 0x40)),
            0x20000 | 0x60000 | 0x10000 | 0x50000 | 0x40000 => {
                Err(StubRead(Trace, 1))
            }
            _ => Err(StubRead(Error, 0)),
        }
    }

    fn w8(&mut self, _offset: u32, _val: u8) -> MemResult<()> {
        Err(StubWrite(Error, ()))
    }

    fn w16(&mut self, offset: u32, val: u16) -> MemResult<()> {
        match offset {
            0x10000 => {
                self.write_address(val);
                Err(StubWrite(Trace, ()))
            }
            0x20000 => {
                self.read_address(val);
                Err(StubWrite(Trace, ()))
            }
            0x00000 | 0x40000 => {
                self.write_data(val);
                Err(StubWrite(Trace, ()))
            }
            _ => Err(StubWrite(Error, ())),
        }
    }

    fn w32(&mut self, offset: u32, val: u32) -> MemResult<()> {
        match offset {
            0x10000 => {
                self.waddr = val;
                self.wph = false;
                Err(StubWrite(Trace, ()))
            }
            0x20000 => {
                self.raddr = val;
                self.rph = false;
                self.drph = false;
                Err(StubWrite(Trace, ()))
            }
            0x00000 | 0x40000 => {
                self.regs.insert(self.waddr, val);
                self.regwrites += 1;
                self.fbwrites += 1;
                Err(StubWrite(Trace, ()))
            }
            _ => Err(StubWrite(Error, ())),
        }
    }
}
