use crate::devices::prelude::*;

// Cypress CY7C68013
#[derive(Debug)]
pub struct Usb5002 {
    // n/a
}

impl Usb5002 {
	pub fn new() -> Usb5002 {
        Usb5002 {
        }
	}
}

impl Device for Usb5002 {
    fn kind(&self) -> &'static str {
        "Usb5002"
    }

    fn probe(&self, offset: u32) -> Probe {
        let reg = match offset {
            0x04 => "Load firmware ?",
            _ => return Probe::Unmapped,
        };

        Probe::Register(reg)
    }
}

impl Memory for Usb5002 {
    fn r32(&mut self, offset: u32) -> MemResult<u32> {
        match offset {
            0x04 => Err(StubRead(Debug, 0)),
            0x08 => Err(StubRead(Debug, 0x0a)),
            _ => Err(Unexpected),
        }
    }

    fn w32(&mut self, offset: u32, _val: u32) -> MemResult<()> {
        match offset {
            0x00 => Err(StubWrite(Debug, ())),
            0x04 => Err(StubWrite(Debug, ())),
            0x0c => Err(StubWrite(Debug, ())),
            0x10 => Err(StubWrite(Debug, ())),
            0x14 => Err(StubWrite(Debug, ())),
            0x18 => Err(StubWrite(Debug, ())),
            0x1c => Err(StubWrite(Debug, ())),
            _ => Err(Unexpected)
        }
    }
}
