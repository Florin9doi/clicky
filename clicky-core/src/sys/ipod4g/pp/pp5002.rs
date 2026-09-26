use std::ops::{Deref, DerefMut};

use crate::devices::{Device, Probe};
use crate::error::*;
use crate::executor::Spawner;
use crate::gui::RenderCallback;
use crate::memory::{MemAccessKind, Memory};
use crate::signal::irq;

use crate::sys::ipod4g::devices;
use crate::sys::ipod4g::{DisplayType, Model};

use super::PpCore;

const FASTRAM_SIZE: usize = 96 * 1024;

/// The main PP5002 memory bus
#[derive(Debug)]
pub struct PP5002Bus {
    pub core: PpCore,
    pub cpucon: devices::CpuCon5002,

    pub scroll: devices::ScrollWheel,
    pub firewire: devices::TSB43AA82,
    pub unk4000: devices::Unk5002_4000,
}

impl Deref for PP5002Bus {
    type Target = PpCore;
    fn deref(&self) -> &PpCore {
        &self.core
    }
}
impl DerefMut for PP5002Bus {
    fn deref_mut(&mut self) -> &mut PpCore {
        &mut self.core
    }
}

impl PP5002Bus {
    pub fn new(
        model: Model,
        task_spawner: Spawner,
        irq_pending: irq::Pending,
        dma_pending: irq::Pending,
        flash_rom: Option<Box<[u8]>>,
    ) -> PP5002Bus {
        let (mut core, i2c_irq_tx) = PpCore::new(
            model,
            task_spawner.clone(),
            irq_pending,
            dma_pending,
            flash_rom,
            FASTRAM_SIZE,
        );
        core.intcon
            .register(1, core.ide_irq_rx.clone())
            // .register(4, ser0_irq_rx)
            // .register(5, i2s_irq_rx)
            // .register(7, ser1_irq_rx)
            .register(11, core.timer1_irq_rx.clone())
            .register(12, core.timer2_irq_rx.clone())
            .register(14, core.gpio0_irq_rx.clone())
            // .register(30, dma_irq_rx) // dma out
            // .register(31, dma_irq_rx) // dma in
            ;

        use devices::*;
        PP5002Bus {
            core,
            cpucon: CpuCon5002::new(),
            scroll: ScrollWheel::new(),
            firewire: TSB43AA82::new(),
            unk4000: Unk5002_4000::new(),
        }
    }

    /// Return a render callback for the panel associated with `model`.
    pub fn render_callback(&self, model: &Model) -> RenderCallback {
        match model.display_type() {
            DisplayType::Mono => self.mlcd.render_callback(),
            _ => panic!("PP5002 only supports mono LCD panels"),
        }
    }
}

macro_rules! mmap {
    (
        RAM {
            $($start_ram:literal $(..= $end_ram:literal)? => $ram:ident,)*
        }
        DEVICES {
            $($start_dev:literal $(..= $end_dev:literal)? => $dev:ident,)*
        }
    ) => {
        macro_rules! impl_mem_r {
            ($fn:ident, $ret:ty) => {
                fn $fn(&mut self, addr: u32) -> MemResult<$ret> {
                    // let mut addr = addr;
                    // if (0x00..0x1F).contains(&addr) && self.cachecon.local_evt {
                    //     addr |= 0x6000_f100;
                    // }

                    let (phys_addr, prot) = self.memcon.virt_to_phys(addr, MemAccessKind::Read);
                    if !prot.r {
                        return Err(MemException::MmuViolation)
                    }

                    match phys_addr {
                        $($start_ram$(..=$end_ram)? => self.$ram.$fn(phys_addr - $start_ram),)*
                        $($start_dev$(..=$end_dev)? => self.$dev.$fn(phys_addr - $start_dev),)*
                        _ => Err(MemException::Unexpected),
                    }
                }
            };
        }

        macro_rules! impl_mem_w {
            ($fn:ident, $val:ty) => {
                fn $fn(&mut self, addr: u32, val: $val) -> MemResult<()> {
                    let (phys_addr, prot) = self.memcon.virt_to_phys(addr, MemAccessKind::Write);
                    if !prot.w {
                        return Err(MemException::MmuViolation)
                    }

                    match phys_addr {
                        $($start_ram$(..=$end_ram)? => self.$ram.$fn(phys_addr - $start_ram, val),)*
                        $($start_dev$(..=$end_dev)? => self.$dev.$fn(phys_addr - $start_dev, val),)*
                        _ => Err(MemException::Unexpected),
                    }
                }
            };
        }

        macro_rules! impl_mem_x {
            ($fn:ident, $ret:ty) => {
                fn $fn(&mut self, addr: u32) -> MemResult<$ret> {
                    // let phys_addr = if (0x00..0x1F).contains(&addr) && self.cachecon.local_evt {
                    //     match self.evp.r32(addr) {
                    //         Ok(val) => val,
                    //         Err(e) => return Err(e),
                    //     }
                    // } else {
                        let (phys_addr, prot) = self.memcon.virt_to_phys(addr, MemAccessKind::Execute);
                        if !prot.x {
                            return Err(MemException::MmuViolation)
                        }
                        // final_addr
                    // };

                    match phys_addr {
                        $($start_ram$(..=$end_ram)? => self.$ram.$fn(phys_addr - $start_ram),)*
                        $($start_dev$(..=$end_dev)? => self.$dev.$fn(phys_addr - $start_dev),)*
                        _ => Err(MemException::Unexpected),
                    }
                }
            };
        }

        impl Device for PP5002Bus {
            fn kind(&self) -> &'static str {
                "PP5002"
            }

            fn probe(&self, addr: u32) -> Probe {
                let (addr, _) = self.memcon.virt_to_phys(addr, MemAccessKind::Read);
                match addr {
                    $($start_ram$(..=$end_ram)? => {
                        Probe::from_device(&self.$ram, addr - $start_ram)
                    })*
                    $($start_dev$(..=$end_dev)? => {
                        Probe::from_device(&self.$dev, addr - $start_dev)
                    })*
                    _ => Probe::Unmapped,
                }
            }
        }

        impl Memory for PP5002Bus {
            impl_mem_r!(r8, u8);
            impl_mem_r!(r16, u16);
            impl_mem_r!(r32, u32);
            impl_mem_w!(w8, u8);
            impl_mem_w!(w16, u16);
            impl_mem_w!(w32, u32);
            impl_mem_x!(x16, u16);
            impl_mem_x!(x32, u32);
        }
    };
}

mmap! {
    RAM {
        0x2800_0000..=0x29ff_ffff => sdram,
        0x4000_0000..=0x4001_7fff => fastram,
    }

    DEVICES {
        // ext
        0x0000_0000..=0x000f_ffff => flash,
        0x3000_0000..=0x3000_01ff => firewire,
        // int
        0xc000_1000..=0xc000_101f => mlcd,
        0xc000_2500..=0xc000_25ff => i2s,
        0xc000_3000..=0xc000_3fff => eidecon,
        0xc000_6000..=0xc000_6020 => serial0,
        0xc000_6040..=0xc000_6060 => serial1,
        0xc000_8000..=0xc000_801f => i2ccon,
        0xc000_8020..=0xc000_803f => total_mystery, // audio?
        0xc400_0000..=0xc400_000f => cpuid,
        0xcf00_0000..=0xcf00_007f => gpio_abcd,
        0xcf00_1000..=0xcf00_10ff => intcon,
        0xcf00_1100..=0xcf00_1107 => timer1,
        0xcf00_1108..=0xcf00_110f => timer2,
        0xcf00_1110..=0xcf00_1113 => usec_timer,
        0xcf00_4000..=0xcf00_401f => unk4000,
        0xcf00_4020..=0xcf00_402f => cachecon,
        0xcf00_4030..=0xcf00_403f => total_mystery, // pp_ver
        0xcf00_4040..=0xcf00_404f => total_mystery, // wmcodec?
        0xcf00_4050..=0xcf00_40ff => cpucon,
        0xcf00_5000..=0xcf00_50ff => devcon,
        0x2a0_00000..=0x2a00_00ff => total_mystery,
        0x2c0_00000..=0x2c00_00ff => total_mystery,
        0xc800_4000..=0xc800_4fff => total_mystery,
        0xf000_0000..=0xf000_ffff => memcon,
    }
}
