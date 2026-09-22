use crate::devices::prelude::*;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use crate::devices::platform::pp::CpuConDevice;

pub use super::common::CpuId;

#[allow(dead_code)]
mod flags {
    type Range = std::ops::RangeInclusive<usize>;
    pub const CPU_SLEEP: usize = 15;
    pub const COP_SLEEP: usize = 14;
}

/// PP5002 CPU controller
#[derive(Debug)]
pub struct CpuCon5002 {
    cpuctl: Arc<AtomicU32>,
    copctl: Arc<AtomicU32>,
}

impl CpuCon5002 {
    pub fn new() -> CpuCon5002 {
        CpuCon5002 {
            cpuctl: Arc::new(0x0000_0000.into()),
            copctl: Arc::new(0x0000_0000.into()),
        }
    }

    pub fn reset(&mut self) {
        self.cpuctl.store(0, Ordering::SeqCst);
        self.copctl.store(0, Ordering::SeqCst);
    }

    pub fn is_cpu_running(&mut self, cpu: CpuId) -> bool {
        match cpu {
            CpuId::Cpu => {return self.cpuctl.load(Ordering::SeqCst).get_bit(flags::CPU_SLEEP) == false;}
            CpuId::Cop => {return self.copctl.load(Ordering::SeqCst).get_bit(flags::COP_SLEEP) == false;}
        };
    }

    pub fn wake_on_interrupt(&mut self, cpu: CpuId) {
        match cpu {
            CpuId::Cpu => {
                let _ = self.cpuctl.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |mut reg| {
                    reg = *reg.set_bit(flags::CPU_SLEEP, false);
                    Some(reg)
                });
            }
            CpuId::Cop => {
                let _ = self.copctl.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |mut reg| {
                    reg = *reg.set_bit(flags::COP_SLEEP, false);
                    Some(reg)
                });
            }
        };
    }
}

impl Device for CpuCon5002 {
    fn kind(&self) -> &'static str {
        "System Controller Block"
    }

    fn probe(&self, offset: u32) -> Probe {
        let reg = match offset {
            0x0 => "CPU Status",
            0x4 => "CPU Control",
            0x8 => "COP Control",
            _ => return Probe::Unmapped,
        };

        Probe::Register(reg)
    }
}

impl Memory for CpuCon5002 {
    fn r32(&mut self, offset: u32) -> MemResult<u32> {
        match offset {
            0x00 => {
                Ok(((self.cpuctl.load(Ordering::SeqCst).get_bit(flags::CPU_SLEEP) as u32) << flags::CPU_SLEEP)
                 | ((self.copctl.load(Ordering::SeqCst).get_bit(flags::COP_SLEEP) as u32) << flags::COP_SLEEP))
            }
            0x18 => Err(StubRead(Warn, 0)),
            _ => Err(Unexpected),
        }
    }

    fn w32(&mut self, offset: u32, val: u32) -> MemResult<()> {
        const SLEEP_CMD: u32 = 0xca;
        const WAKE_CMD: u32 = 0xce;
        match offset {
            0x00 => Err(StubWrite(Error, ())),
            0x04 => { // cpu control
                match val {
                    SLEEP_CMD => {
                        let _ = self.copctl.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |mut reg| {
                            reg = *reg.set_bit(flags::CPU_SLEEP, true);
                            Some(reg)
                        });
                    }
                    WAKE_CMD => {
                        let _ = self.copctl.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |mut reg| {
                            reg = *reg.set_bit(flags::CPU_SLEEP, false);
                            Some(reg)
                        });
                    }
                    _ => {}
                }
                Ok(())
            }
            0x08 => { // cop control
                match val {
                    SLEEP_CMD => {
                        let _ = self.copctl.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |mut reg| {
                            reg = *reg.set_bit(flags::COP_SLEEP, true);
                            Some(reg)
                        });
                    }
                    WAKE_CMD => {
                        let _ = self.copctl.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |mut reg| {
                            reg = *reg.set_bit(flags::COP_SLEEP, false);
                            Some(reg)
                        });
                    }
                    _ => {}
                }
                Ok(())
            },
            0x0c => Err(StubWrite(Error, ())),
            0x10 => Err(StubWrite(Error, ())),
            0x14 => Err(StubWrite(Error, ())),
            0x18 => Err(StubWrite(Error, ())),
            _ => Err(Unexpected),
        }
    }

    fn w16(&mut self, offset: u32, val: u16) -> MemResult<()> {
        self.w32(offset, val as u32)
    }
}

impl CpuConDevice for CpuCon5002 {
    fn reset(&mut self) { self.reset() }
    fn is_cpu_running(&mut self, cpu: CpuId) -> bool { self.is_cpu_running(cpu) }
    fn wake_on_interrupt(&mut self, cpu: CpuId) { self.wake_on_interrupt(cpu) }
}
