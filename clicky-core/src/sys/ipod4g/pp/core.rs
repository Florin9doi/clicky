use crate::devices::platform::pp::common::CpuId;
use crate::executor::Spawner;
use crate::signal::irq;

use crate::sys::ipod4g::devices;
use crate::sys::ipod4g::irq::Receiver;
use crate::sys::ipod4g::ArcMutexDevice;
use crate::sys::ipod4g::Model;

/// Peripherals common to every PortalPlayer SoC
#[derive(Debug)]
pub struct PpCore {
    pub sdram: devices::AsanRam,
    pub fastram: devices::AsanRam,
    pub cpuid: devices::CpuIdReg,
    pub flash: devices::Flash,
    pub timer1: devices::CfgTimer,
    pub timer2: devices::CfgTimer,
    pub usec_timer: devices::UsecTimer,
    pub firewire: devices::Firewire,
    pub i2ccon: devices::I2CCon,
    pub i2s: devices::I2SCon,
    pub ppcon: devices::PPCon,
    pub devcon: devices::DevCon,
    pub intcon: devices::IntCon,
    pub eidecon: devices::EIDECon,
    pub memcon: devices::MemCon,
    pub mlcd: devices::MonoLcdBridge,
    pub cachecon: devices::CacheCon,
    pub gpio_abcd: ArcMutexDevice<devices::GpioBlock>,
    pub serial0: devices::Serial,
    pub serial1: devices::Serial,
    pub evp: devices::Evp,
    pub rtc: devices::Rtc,

    pub gpio0_irq_rx: Receiver,
    pub ide_irq_rx: Receiver,
    pub timer1_irq_rx: Receiver,
    pub timer2_irq_rx: Receiver,
    pub i2c_irq_rx: Receiver,
    pub ide_dmarq_rx: Receiver,

    pub mystery_irq_con: devices::Stub,
    pub mystery_lcd_con: devices::Stub,
    pub total_mystery: devices::Stub,
}

impl PpCore {
    pub fn new(
        model: Model,
        task_spawner: Spawner,
        irq_pending: irq::Pending,
        dma_pending: irq::Pending,
        flash_rom: Option<Box<[u8]>>,
        fastram_size: usize,
    ) -> (PpCore, irq::Sender) {
        let (gpio0_irq_tx, gpio0_irq_rx) = irq::new(irq_pending.clone(), "GPIO0");
        let (ide_irq_tx, ide_irq_rx) = irq::new(irq_pending.clone(), "IDE");
        let (timer1_irq_tx, timer1_irq_rx) = irq::new(irq_pending.clone(), "Timer1");
        let (timer2_irq_tx, timer2_irq_rx) = irq::new(irq_pending.clone(), "Timer2");
        let (i2c_irq_tx, i2c_irq_rx) = irq::new(irq_pending, "I2C");
        let (ide_dmarq_tx, ide_dmarq_rx) = irq::new(dma_pending.clone(), "IDE DMA");

        let mut i2ccon = devices::I2CCon::new(i2c_irq_tx.clone());
        i2ccon.register_device(0x08, Box::new(devices::i2c::Pcf5060x::new()));

        use devices::*;
        let core = PpCore {
            sdram: AsanRam::new(32 * 1024 * 1024, true), // 32 MB
            fastram: AsanRam::new(fastram_size, true),
            cpuid: CpuIdReg::new(),
            flash: match flash_rom {
                Some(dump) => Flash::new_with_dump(dump).expect("invalid flash dump"),
                None => Flash::new(),
            },
            timer1: CfgTimer::new("1", timer1_irq_tx, task_spawner.clone()),
            timer2: CfgTimer::new("2", timer2_irq_tx, task_spawner),
            usec_timer: UsecTimer::new(),
            firewire: Firewire::new(),
            i2ccon,
            i2s: I2SCon::new(),
            ppcon: PPCon::new(),
            devcon: DevCon::new(),
            intcon: IntCon::new(),
            eidecon: EIDECon::new(ide_irq_tx, ide_dmarq_tx),
            memcon: MemCon::new(),
            mlcd: MonoLcdBridge::new(model.make_panel()),
            cachecon: CacheCon::new(),
            gpio_abcd: ArcMutexDevice::new(devices::GpioBlock::new(gpio0_irq_tx, ["A", "B", "C", "D"])),
            serial0: Serial::new("0"),
            serial1: Serial::new("1"),
            evp: Evp::new(),
            rtc: Rtc::new(),
            gpio0_irq_rx,
            ide_irq_rx,
            timer1_irq_rx,
            timer2_irq_rx,
            i2c_irq_rx,
            ide_dmarq_rx,

            mystery_irq_con: Stub::new("Mystery IRQ Con?"),
            mystery_lcd_con: Stub::new("Mystery LCD Con?"),
            total_mystery: Stub::new("(?) Arbiter Priority"),
        };
        (core, i2c_irq_tx)
    }

    pub fn set_cpuid(&mut self, cpuid: CpuId) {
        self.cpuid.set_cpuid(cpuid);
        self.memcon.set_cpuid(cpuid);
    }
}
