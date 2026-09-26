use std::ops::{Deref, DerefMut};

use crate::devices::util::ArcMutexDevice;
use crate::devices::{Device, Probe};
use crate::error::*;
use crate::executor::Spawner;
use crate::gui::RenderCallback;
use crate::memory::{MemAccessKind, Memory};
use crate::signal::irq;

use crate::sys::ipod4g::devices;
use crate::sys::ipod4g::{DisplayType, Model, SoC};

use super::PpCore;

const PP5020_FASTRAM: usize =  96 * 1024;
const PP5022_FASTRAM: usize = 128 * 1024;

/// The main PP5002 memory bus
#[derive(Debug)]
pub struct PP502xBus {
    pub core: PpCore,
    pub cpucon: devices::CpuCon,

    pub scroll: devices::ScrollWheel,
    pub opto: devices::OptoWheel,
    pub clcd: devices::ColorLcdBridge,
    pub usb: devices::Usb,
    pub gpio_efgh: ArcMutexDevice<devices::GpioBlock>,
    pub gpio_ijkl: ArcMutexDevice<devices::GpioBlock>,
    pub gpio_mirror_abcd: devices::GpioBlockAtomicMirror,
    pub gpio_mirror_efgh: devices::GpioBlockAtomicMirror,
    pub gpio_mirror_ijkl: devices::GpioBlockAtomicMirror,
    pub mailbox: devices::Mailbox,
    pub dmacon0: devices::DmaCon,
    pub dmacon1: devices::DmaCon,

    pub mystery_flash_stub: devices::Stub,
    pub pwmcon: devices::PWMCon,
    pub bcm_video: devices::Bcm2722,

    pub pp5002_serial_stub: devices::Stub,
}

impl Deref for PP502xBus {
    type Target = PpCore;
    fn deref(&self) -> &PpCore {
        &self.core
    }
}
impl DerefMut for PP502xBus {
    fn deref_mut(&mut self) -> &mut PpCore {
        &mut self.core
    }
}

impl PP502xBus {
    // #[allow(clippy::redundant_clone)] // Makes the code cleaner in this case
    pub fn new(
        model: Model,
        task_spawner: Spawner,
        irq_pending: irq::Pending,
        dma_pending: irq::Pending,
        flash_rom: Option<Box<[u8]>>,
    ) -> PP502xBus {
        let (gpio1_irq_tx, gpio1_irq_rx) = irq::new(irq_pending.clone(), "GPIO1");
        let (gpio2_irq_tx, gpio2_irq_rx) = irq::new(irq_pending.clone(), "GPIO2");
        // mailbox is the only core-specific IRQ in the system, which is kinda neat
        let (mbx_cpu_irq_tx, mbx_cpu_irq_rx) = irq::new(irq_pending.clone(), "Mailbox (CPU)");
        let (mbx_cop_irq_tx, mbx_cop_irq_rx) = irq::new(irq_pending.clone(), "Mailbox (COP)");
        let fastram_size = match model.soc {
            SoC::Pp5020 => { PP5020_FASTRAM },
            SoC::Pp5022 => { PP5022_FASTRAM },
            _ => panic!("unsupported SoC"),
        };

        let (mut core, i2c_irq_tx) = PpCore::new(
            model,
            task_spawner.clone(),
            irq_pending,
            dma_pending,
            flash_rom,
            fastram_size,
        );
        core.intcon
            .register(0, core.timer1_irq_rx.clone())
            .register(1, core.timer2_irq_rx.clone())
            .register_core_specific(4, mbx_cpu_irq_rx, mbx_cop_irq_rx)
            // .register(10, i2s_irq_rx)
            // .register(20, usb_irq_rx)
            .register(23, core.ide_irq_rx.clone())
            // .register(25, firewire_irq_rx)
            // .register(26, dma_irq_rx)
            .register(32, core.gpio0_irq_rx.clone())
            .register(33, gpio1_irq_rx)
            .register(34, gpio2_irq_rx)
            // .register(36, ser0_irq_rx)
            // .register(37, ser1_irq_rx)
            .register(40, core.i2c_irq_rx.clone());

        let gpio_efgh = ArcMutexDevice::new(devices::GpioBlock::new(gpio1_irq_tx, ["E", "F", "G", "H"]));
        let gpio_ijkl = ArcMutexDevice::new(devices::GpioBlock::new(gpio2_irq_tx, ["I", "J", "K", "L"]));
        let gpio_mirror_abcd = core.gpio_abcd.clone();
        let gpio_mirror_efgh = gpio_efgh.clone();
        let gpio_mirror_ijkl = gpio_ijkl.clone();
        let dmacon0 = DmaCon::new("0", Some(core.ide_dmarq_rx.clone()));
        // the undocumented second engine -- nothing routes DMA requests to it yet
        let dmacon1 = DmaCon::new("1", None);

        use devices::*;
        PP502xBus {
            core,
            cpucon: CpuCon::new(task_spawner.clone()),
            mailbox: Mailbox::new(mbx_cpu_irq_tx, mbx_cop_irq_tx),
            clcd: ColorLcdBridge::new(model.make_panel()),
            gpio_efgh,
            gpio_ijkl,
            gpio_mirror_abcd: GpioBlockAtomicMirror::new(gpio_mirror_abcd),
            gpio_mirror_efgh: GpioBlockAtomicMirror::new(gpio_mirror_efgh),
            gpio_mirror_ijkl: GpioBlockAtomicMirror::new(gpio_mirror_ijkl),
            dmacon0,
            dmacon1,
            scroll: ScrollWheel::new(),
            opto: OptoWheel::new(i2c_irq_tx),
            pwmcon: PWMCon::new(),
            usb: Usb::new(),

            mystery_flash_stub: Stub::new("Mystery FlashROM Con?"),
            bcm_video: Bcm2722::new(model.make_panel()),
            pp5002_serial_stub: Stub::new("PP5002 serial stub"),
        }
    }

    /// Return a render callback for the panel associated with `model`.
    pub fn render_callback(&self, model: &Model) -> RenderCallback {
        match model.display_type() {
            DisplayType::Mono => self.mlcd.render_callback(),
            DisplayType::Color | DisplayType::Hd66789 => self.clcd.render_callback(),
            DisplayType::Bcm2722 => self.bcm_video.render_callback(),
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
                    let mut addr = addr;
                    if (0x00..0x1F).contains(&addr) && self.cachecon.local_evt {
                        addr |= 0x6000_f100;
                    }

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
                    let phys_addr = if (0x00..0x1F).contains(&addr) && self.cachecon.local_evt {
                        match self.evp.r32(addr) {
                            Ok(val) => val,
                            Err(e) => {
                                return Err(e);
                            }
                        }
                    } else {
                        let (final_addr, prot) = self.memcon.virt_to_phys(addr, MemAccessKind::Execute);
                        if !prot.x {
                            return Err(MemException::MmuViolation)
                        }
                        final_addr
                    };

                    match phys_addr {
                        $($start_ram$(..=$end_ram)? => self.$ram.$fn(phys_addr - $start_ram),)*
                        $($start_dev$(..=$end_dev)? => self.$dev.$fn(phys_addr - $start_dev),)*
                        _ => Err(MemException::Unexpected),
                    }
                }
            };
        }

        impl Device for PP502xBus {
            fn kind(&self) -> &'static str {
                "PP505x"
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

        impl Memory for PP502xBus {
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
        0x1000_0000..=0x11ff_ffff => sdram,
        0x1200_0000..=0x12ff_ffff => sdram, // mirror
        0x4000_0000..=0x4001_ffff => fastram,
    }

    DEVICES {
        0x0000_0000..=0x000f_ffff => flash,
        0x3000_0000..=0x3007_ffff => bcm_video, // 5g normal mode
        0xb000_0000..=0xb007_ffff => bcm_video, // 5g diag mode
        0x6000_0000..=0x6000_0fff => cpuid,
        0x6000_1000..=0x6000_102f => mailbox,
        0x6000_4000..=0x6000_41ff => intcon,
        0x6000_5000..=0x6000_5007 => timer1,
        0x6000_5008..=0x6000_500f => timer2,
        0x6000_5010..=0x6000_5013 => usec_timer,
        0x6000_5014..=0x6000_5017 => rtc,
        0x6000_6000..=0x6000_6fff => devcon,
        0x6000_7000..=0x6000_7fff => cpucon,
        // Memory accesses to dmacon1 are suspiciously similar to dmacon0
        0x6000_8000..=0x6000_9fff => dmacon1,
        0x6000_a000..=0x6000_bfff => dmacon0,
        0x6000_c000..=0x6000_cfff => cachecon,
        0x6000_d000..=0x6000_d07f => gpio_abcd,
        0x6000_d080..=0x6000_d0ff => gpio_efgh,
        0x6000_d100..=0x6000_d17f => gpio_ijkl,
        0x6000_d800..=0x6000_d87f => gpio_mirror_abcd,
        0x6000_d880..=0x6000_d8ff => gpio_mirror_efgh,
        0x6000_d900..=0x6000_d97f => gpio_mirror_ijkl,

        0x6400_4000..=0x6400_41ff => intcon, // i guess there's a mirror?

        0x7000_0000..=0x7000_1fff => ppcon,
        0x7000_3000..=0x7000_301f => mlcd,
        0x7000_6000..=0x7000_603f => serial0,
        0x7000_6040..=0x7000_607f => serial1,
        0x7000_8a00..=0x7000_8b0f => clcd,
        0x7000_a000..=0x7000_a03f => pwmcon,
        0x7000_c000..=0x7000_c0ff => i2ccon,
        0x7000_c100..=0x7000_c1ff => opto,
        0x7000_2800..=0x7000_28ff => i2s,
        0xc300_0000..=0xc300_0fff => eidecon,
        0xf000_0000..=0xf000_ffff => memcon,

        0x6000_f000..=0x6000_f01f => evp, // Tegra drivers mention 0x6000F1xx but 0x6000F0xx is mentioned in PP5020 RE litterature
        0x6000_f100..=0x6000_f11f => evp, // I assume 0x6000F0xx and 0x6000F1xx are mirrored? Maybe one is used for the main CPU,
                                            // the other is used for COP?

        // all the stubs

        0x6000_1038 => mystery_irq_con,
        0x6000_111c => mystery_irq_con,
        0x6000_1128 => mystery_irq_con,
        0x6000_1138 => mystery_irq_con,

        // Four registers at +0x00/+0x04/+0x08/+0x0c, RetailOS programs them
        // from one straight-line routine, never touched in diags.
        //
        // The values it writes are a cyclic Latin square:
        //
        //           +0x00  +0x04  +0x08  +0x0c
        //   bits0-1   0      1      2      3
        //   bits2-3   3      0      1      2
        //   bits4-5   2      3      0      1
        //   bits6-7   1      2      3      0
        //
        // Undocumented everywhere. Arbiter priority matrix of Multi Path Mem
        // Controller?
        0x6000_3000..=0x6000_30ff => total_mystery,
        0x7000_2c00 => total_mystery,
        0x7000_c300..=0x7000_c3ff => total_mystery, // triggered by 5g with nor but no hdd
        // Diagnostics program reads from address, and write back 0x10000000
        0x7000_3800 => total_mystery,
        0xc031_b1d8 => mystery_flash_stub,
        0xc031_b1e8 => mystery_flash_stub,
        0xc500_0000..=0xc500_01ff => usb,
        0xc600_0000..=0xc600_01ff => firewire,
        0xffff_fe00..=0xffff_ffff => mystery_flash_stub,

        // PP5002 addresses, I know, but iPodLinux uses that
        0xc000_6000..=0xc000_6020 => pp5002_serial_stub,
        0xc000_6040..=0xc000_6060 => pp5002_serial_stub,
    }
}
