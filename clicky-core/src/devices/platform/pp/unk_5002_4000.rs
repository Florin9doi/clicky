use crate::devices::prelude::*;

#[derive(Debug)]
pub struct Unk5002_4000 {
    // n/a
}

impl Unk5002_4000 {
	pub fn new() -> Unk5002_4000 {
        Unk5002_4000 {}
	}
}

impl Device for Unk5002_4000 {
    fn kind(&self) -> &'static str {
        "Unk5002_4000"
    }

    fn probe(&self, offset: u32) -> Probe {
        let reg = match offset {
            0x00..=0x1f => "Unk",
            _ => return Probe::Unmapped,
        };

        Probe::Register(reg)
    }
}

impl Memory for Unk5002_4000 {
    fn r32(&mut self, offset: u32) -> MemResult<u32> {
        match offset {
            0x00 => Err(StubRead(Trace, 0)),
            0x08 => Err(StubRead(Trace, 0)),
            0x10 => Ok(0x0000_0800),
            0x1c => Err(StubRead(Trace, 0)),
            _ => Err(Unexpected),
        }
    }

    fn w32(&mut self, offset: u32, _val: u32) -> MemResult<()> {
        match offset {
            0x00 => Err(StubWrite(Trace, ())),
            0x08 => Err(StubWrite(Trace, ())),
            0x10 => Err(StubWrite(Trace, ())),
            0x14 => Err(StubWrite(Trace, ())),
            0x1c => Err(StubWrite(Trace, ())),
            _ => Err(Unexpected)
        }
    }
}
