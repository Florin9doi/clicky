use crate::devices::i2c::prelude::*;

use std::convert::TryFrom;

use chrono::{Datelike, Local, Timelike};
use num_enum::TryFromPrimitive;

#[derive(Debug)]
pub struct Adv7174 {
    last_op_was_write: bool,
    register: Option<u8>,
}

impl Adv7174 {
    pub fn new() -> Adv7174 {
        Adv7174 {
            last_op_was_write: false,
            register: None,
        }
    }
}

impl Device for Adv7174 {
    fn kind(&self) -> &'static str {
        "Adv7174"
    }

    fn probe(&self, _offset: u32) -> Probe {
        let reg = match self.register {
            Some(reg) => reg,
            None => return Probe::Register("<no register selected>"),
        };
        let reg = match reg {
            0x00 => "Mode Register 0",
            0x01 => "Mode Register 1",
            0x02 => "Mode Register 2",
            0x03 => "Mode Register 3",
            0x04 => "Mode Register 4",
            0x07 => "Timing Mode Register 0",
            0x08 => "Timing Mode Register 1",
            0x09 => "Subcarrier Freq 0",
            0x0a => "Subcarrier Freq 1",
            0x0b => "Subcarrier Freq 2",
            0x0c => "Subcarrier Freq 3",
            0x0d => "Subcarrier Phase",
            0x0e => "CC Ext 0",
            0x0f => "CC Ext 1",
            0x10 => "CC 0",
            0x11 => "CC 1",
            _ => "<invalid>",
        };

        Probe::Register(reg)
    }
}

impl I2CDevice for Adv7174 {
    fn read(&mut self) -> MemResult<u8> {
        self.last_op_was_write = false;

        match self.register {
            None => Err(Fatal("no register specified for read".into())),
            Some(ref mut reg) => {
                // let reg = Reg::try_from(reg_).map_err(|_| Fatal("invalid register".into()))?;
                // self.inner.read(reg)
                Err(StubRead(Error, 0))
            }
        }
    }

    fn write(&mut self, data: u8) -> MemResult<()> {
        if !self.last_op_was_write {
            self.register = None;
        }
        self.last_op_was_write = true;

        match self.register {
            None => Ok(self.register = Some(data)),
            Some(ref mut reg) => {
                // let reg = Reg::try_from(reg_).map_err(|_| Fatal("invalid register".into()))?;
                // self.inner.write(reg, data)
                Err(StubWrite(Error, ()))
            }
        }
    }

    fn write_done(&mut self) -> MemResult<()> {
        self.last_op_was_write = false;
        Ok(())
    }
}
