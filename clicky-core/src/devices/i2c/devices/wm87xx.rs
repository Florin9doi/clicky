use crate::devices::i2c::prelude::*;

use std::convert::TryFrom;

use chrono::{Datelike, Local, Timelike};
use num_enum::TryFromPrimitive;

#[derive(Debug)]
pub struct Wm87xx {
    last_op_was_write: bool,
    register: Option<u8>,
}

impl Wm87xx {
    pub fn new() -> Wm87xx {
        Wm87xx {
            last_op_was_write: false,
            register: None,
        }
    }
}

impl Device for Wm87xx {
    fn kind(&self) -> &'static str {
        "Wm87xx"
    }

    fn probe(&self, _offset: u32) -> Probe {
        let reg = match self.register {
            Some(reg) => reg,
            None => return Probe::Register("<no register selected>"),
        };
        let reg = match reg {
            0x00 => "Left Line In",
            0x01 => "Right Line In",
            0x02 => "Left Headphone Out",
            0x03 => "Right Headphone Out",
            0x04 => "Analogue Audio Path Control",
            0x05 => "Digital Audio Path Control",
            0x06 => "Power Down Control",
            0x07 => "Digital Audio Interface Format",
            0x08 => "Sampling Control",
            0x09 => "Active Control",
            0x0f => "Reset Register",
            _ => "<invalid>",
        };

        Probe::Register(reg)
    }
}

impl I2CDevice for Wm87xx {
    fn read(&mut self) -> MemResult<u8> {
        self.last_op_was_write = false;

        match self.register {
            None => Err(Fatal("no register specified for read".into())),
            Some(ref mut reg) => {
                // let reg = Reg::try_from(reg_).map_err(|_| Fatal("invalid register".into()))?;
                // self.inner.read(reg)
                Err(StubRead(Trace, 0))
            }
        }
    }

    fn write(&mut self, data: u8) -> MemResult<()> {
        if !self.last_op_was_write {
            self.register = None;
        }
        self.last_op_was_write = true;

        match self.register {
            None => Ok(self.register = Some(data >> 1)),
            Some(ref mut reg) => {
                // let reg = Reg::try_from(reg_).map_err(|_| Fatal("invalid register".into()))?;
                // self.inner.write(reg, data)
                Err(StubWrite(Trace, ()))
            }
        }
    }

    fn write_done(&mut self) -> MemResult<()> {
        self.last_op_was_write = false;
        Ok(())
    }
}
