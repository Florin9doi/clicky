use std::io::{Read, Seek};
use std::time::Duration;
use std::collections::HashMap;

use armv4t_emu::{reg, Cpu};
use relativity::Timeout;
use thiserror::Error;

use crate::block::BlockDev;
use crate::devices::{display::LcdPanel, Device, Probe};
use crate::error::*;
use crate::executor::*;
use crate::gui::RenderCallback;
use crate::memory::{armv4t_adaptor::MemoryAdapter, MemAccess, Memory};
use crate::signal::{self, gpio, irq};

mod controls;
mod gdb;
mod hle_bootloader;
mod pp;

pub use controls::{Ipod4gBinds, Ipod4gKey};
pub use gdb::Ipod4gGdb;

use hle_bootloader::run_hle_bootloader;

use crate::devices::platform::pp::common::*;
use crate::devices::util::{ArcMutexDevice, MemSniffer};
mod devices {
    pub mod i2c {
        pub use crate::devices::i2c::devices::Pcf5060x;
    }

    pub use crate::devices::{
        display::hd66753::Hd66753,
        display::hd66xxx::Hd66xxx,
        display::hd66789::Hd66789,
        display::bcm2722::Bcm2722,
        display::bcm2722_panel::Bcm2722Panel,
        generic::{ide, AsanRam, Stub},
        platform::pp::*,
    };
}

/// How "--hold-keys" are held down for
const BOOT_HOLD_DURATION: Duration = Duration::from_millis(3000);

enum BlockMode {
    Blocking,
    NonBlocking,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayType {
    Mono,
    Color,
    Hd66789,
    Bcm2722,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplaySize {
    pub width: usize,
    pub height: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoC {
    Pp5002,
    Pp5020,
    Pp5022,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpioBlockId {
    Abcd,
    Efgh,
    Ijkl,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySticky {
    False,
    True,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyActive {
    High,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRoute {
    ClickWheel,
    Gpio(GpioBlockId, u8, KeySticky, KeyActive),
}

const IPOD_1G_KEYMAP: &[(Ipod4gKey, KeyRoute)] = &[
    (Ipod4gKey::Right,  KeyRoute::Gpio(GpioBlockId::Abcd, 0, KeySticky::False, KeyActive::Low)),
    (Ipod4gKey::Action, KeyRoute::Gpio(GpioBlockId::Abcd, 1, KeySticky::False, KeyActive::Low)),
    (Ipod4gKey::Down,   KeyRoute::Gpio(GpioBlockId::Abcd, 2, KeySticky::False, KeyActive::Low)),
    (Ipod4gKey::Left,   KeyRoute::Gpio(GpioBlockId::Abcd, 3, KeySticky::False, KeyActive::Low)),
    (Ipod4gKey::Up,     KeyRoute::Gpio(GpioBlockId::Abcd, 4, KeySticky::False, KeyActive::Low)),
    (Ipod4gKey::Hold,   KeyRoute::Gpio(GpioBlockId::Abcd, 5, KeySticky::True,  KeyActive::High)),
];
const IPOD_3G_KEYMAP: &[(Ipod4gKey, KeyRoute)] = &[
    (Ipod4gKey::Right,  KeyRoute::Gpio(GpioBlockId::Abcd, 0, KeySticky::False, KeyActive::Low)),
    (Ipod4gKey::Action, KeyRoute::Gpio(GpioBlockId::Abcd, 1, KeySticky::False, KeyActive::Low)),
    (Ipod4gKey::Down,   KeyRoute::Gpio(GpioBlockId::Abcd, 2, KeySticky::False, KeyActive::Low)),
    (Ipod4gKey::Left,   KeyRoute::Gpio(GpioBlockId::Abcd, 3, KeySticky::False, KeyActive::Low)),
    (Ipod4gKey::Up,     KeyRoute::Gpio(GpioBlockId::Abcd, 4, KeySticky::False, KeyActive::Low)),
    (Ipod4gKey::Hold,   KeyRoute::Gpio(GpioBlockId::Abcd, 5, KeySticky::True,  KeyActive::Low)),
];
const IPOD_MINI1G_KEYMAP: &[(Ipod4gKey, KeyRoute)] = &[
    (Ipod4gKey::Action, KeyRoute::Gpio(GpioBlockId::Abcd, 0, KeySticky::False, KeyActive::Low)),
    (Ipod4gKey::Up,     KeyRoute::Gpio(GpioBlockId::Abcd, 1, KeySticky::False, KeyActive::Low)),
    (Ipod4gKey::Down,   KeyRoute::Gpio(GpioBlockId::Abcd, 2, KeySticky::False, KeyActive::Low)),
    (Ipod4gKey::Right,  KeyRoute::Gpio(GpioBlockId::Abcd, 3, KeySticky::False, KeyActive::Low)),
    (Ipod4gKey::Left,   KeyRoute::Gpio(GpioBlockId::Abcd, 4, KeySticky::False, KeyActive::Low)),
    (Ipod4gKey::Hold,   KeyRoute::Gpio(GpioBlockId::Abcd, 5, KeySticky::True,  KeyActive::Low)),
];
const CLICKWHEEL_KEYMAP: &[(Ipod4gKey, KeyRoute)] = &[
    (Ipod4gKey::Action, KeyRoute::ClickWheel),
    (Ipod4gKey::Up,     KeyRoute::ClickWheel),
    (Ipod4gKey::Down,   KeyRoute::ClickWheel),
    (Ipod4gKey::Left,   KeyRoute::ClickWheel),
    (Ipod4gKey::Right,  KeyRoute::ClickWheel),
    (Ipod4gKey::Hold,   KeyRoute::Gpio(GpioBlockId::Abcd, 5, KeySticky::True,  KeyActive::Low)),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    pub name: &'static str,
    pub alias: &'static str,
    pub soc: SoC,
    pub display_type: DisplayType,
    pub mirrored: bool,
    pub width: usize,
    pub height: usize,
    pub keymap: &'static [(Ipod4gKey, KeyRoute)],
}

impl Model {
    pub const ALL: &[Self] = &[
        Self {
            name: "iPod (1st gen)",
            alias: "1g",
            soc: SoC::Pp5002,
            display_type: DisplayType::Mono,
            mirrored: false,
            width: 160,
            height: 128,
            keymap: IPOD_1G_KEYMAP,
        },
        Self {
            name: "iPod (3rd gen)",
            alias: "3g",
            soc: SoC::Pp5002,
            display_type: DisplayType::Mono,
            mirrored: false,
            width: 160,
            height: 128,
            keymap: IPOD_3G_KEYMAP,
        },
        Self {
            name: "iPod (4th gen)",
            alias: "4gmono",
            soc: SoC::Pp5020,
            display_type: DisplayType::Mono,
            mirrored: false,
            width: 160,
            height: 128,
            keymap: CLICKWHEEL_KEYMAP,
        },
        Self {
            name: "iPod Photo (early)",
            alias: "4gphoto",
            soc: SoC::Pp5020,
            display_type: DisplayType::Color,
            mirrored: false,
            width: 220,
            height: 176,
            keymap: CLICKWHEEL_KEYMAP,
        },
        Self {
            name: "iPod Color (late)",
            alias: "4gcolor",
            soc: SoC::Pp5020,
            display_type: DisplayType::Hd66789,
            mirrored: false,
            width: 220,
            height: 176,
            keymap: CLICKWHEEL_KEYMAP,
        },
        Self {
            name: "iPod Video",
            alias: "5gvideo",
            soc: SoC::Pp5020,
            display_type: DisplayType::Bcm2722,
            mirrored: false,
            width: 320,
            height: 240,
            keymap: CLICKWHEEL_KEYMAP,
        },
        Self {
            name: "iPod Mini (1st gen)",
            alias: "mini1g",
            soc: SoC::Pp5020,
            display_type: DisplayType::Mono,
            mirrored: true,
            width: 138,
            height: 110,
            keymap: IPOD_MINI1G_KEYMAP,
        },
        Self {
            name: "iPod Mini (2nd gen)",
            alias: "mini2g",
            soc: SoC::Pp5022,
            display_type: DisplayType::Mono,
            mirrored: true,
            width: 138,
            height: 110,
            keymap: CLICKWHEEL_KEYMAP,
        },
        Self {
            name: "iPod Nano",
            alias: "nano1g",
            soc: SoC::Pp5020,
            display_type: DisplayType::Hd66789,
            mirrored: false,
            width: 176,
            height: 132,
            keymap: CLICKWHEEL_KEYMAP,
        },
    ];

    pub fn from_str(s: &str) -> Self {
        Self::ALL
            .iter()
            .find(|model| model.alias == s)
            .copied()
            .unwrap_or(Self::ALL[2]) // 4gmono
    }

    pub fn display_type(self) -> DisplayType {
        self.display_type
    }

    pub fn display_size(self) -> DisplaySize {
        DisplaySize {
            width: self.width,
            height: self.height,
        }
    }

    pub fn make_panel(&self) -> Box<dyn LcdPanel> {
        use devices::{Bcm2722Panel, Hd66753, Hd66789, Hd66xxx};
        match self.display_type() {
            DisplayType::Mono => Box::new(Hd66753::new(self.mirrored)),
            DisplayType::Color => Box::new(Hd66xxx::new()),
            DisplayType::Hd66789 => Box::new(Hd66789::new()),
            DisplayType::Bcm2722 => Box::new(Bcm2722Panel::new()),
        }
    }
}

pub enum BootKind<F: Read + Seek> {
    ColdBoot,
    HLEBoot { fw_file: F },
}

#[derive(Debug)]
struct Ipod4gControls {
    controls: devices::Controls<signal::Master>,
    keys: HashMap<Ipod4gKey, controls::KeySink>,
}

/// An iPod system
#[derive(Debug)]
pub struct System {
    pub model: Model,
    frozen: bool,         // set after a fatal error to enable post-mortem debugging
    skip_irq_check: bool, // set by the GDB stub when single-stepping though code

    cpu: Cpu,
    cop: Cpu,
    devices: Bus,
    controls: Option<Ipod4gControls>,
    /// A second set of key sinks, used to synthesize key presses
    /// independently of whoever took ownership of the system's controls.
    synthetic_controls: HashMap<Ipod4gKey, controls::KeySink>,
    /// Keys to hold down once the system starts executing code.
    boot_hold: Option<Vec<Ipod4gKey>>,

    irq_pending: irq::Pending,
    dma_pending: irq::Pending,
    gpio_changed: gpio::Changed,
    i2c_changed: signal::Trigger,
    reset_requested: std::sync::Arc<std::sync::atomic::AtomicBool>,

    executor: Executor,
}

#[derive(Debug)]
pub enum Bus {
    Pp5002(pp::PP5002Bus),
    Pp502x(pp::PP502xBus),
}

/// Helper function for calling vectors of EVP
fn vector_via_evp(core: &mut Cpu, devices: &Bus, vector: u32) {
    let cachecon_local_evt = match devices {
        Bus::Pp5002(bus) => bus.cachecon.local_evt,
        Bus::Pp502x(bus) => bus.cachecon.local_evt,
    };
    if cachecon_local_evt {
        core.reg_set(core.mode(), reg::PC, vector);
    }
}

#[derive(Error, Debug)]
pub enum Ipod4gBuildError {
    #[error("invalid flash dump: {0}")]
    InvalidDump(&'static str),
    #[error("HLE bootloader failed! {0}")]
    HleBootloader(#[from] hle_bootloader::HleBootloaderError),
}

impl System {
    /// Returns a new System instance.
    pub fn new<F>(
        hdd: Box<dyn BlockDev>,
        flash_rom: Option<Box<[u8]>>,
        boot_kind: BootKind<F>,
        model: Model,
    ) -> Result<System, Ipod4gBuildError>
    where
        F: Read + Seek,
    {
        let executor = Executor::new().expect("failed to create task executor");

        // initialize base system
        let irq_pending = irq::Pending::new();
        let dma_pending = irq::Pending::new();
        let gpio_changed = gpio::Changed::new();
        let i2c_changed = signal::Trigger::new(signal::TriggerKind::Edge);

        // hook-up external controls
        let (hold_tx, hold_rx) = gpio::new(gpio_changed.clone(), "Hold");
        let (controls_tx, controls_rx) = devices::Controls::new_tx_rx(i2c_changed.clone());

        let mut sys = System {
            model: model,
            frozen: false,
            skip_irq_check: false,

            cpu: Cpu::new(),
            cop: Cpu::new(),
            devices: Bus::new(
                executor.spawner(),
                irq_pending.clone(),
                dma_pending.clone(),
                model,
                flash_rom,
            ),
            controls: None,
            synthetic_controls: HashMap::new(),
            boot_hold: None,

            irq_pending,
            dma_pending,
            gpio_changed: gpio_changed.clone(),
            i2c_changed: i2c_changed.clone(),
            reset_requested: Default::default(),

            executor,
        };

        sys.reset_requested = sys.devices.devcon().reset_requested();

        // buttons
        let mut keys: HashMap<Ipod4gKey, controls::KeySink> = HashMap::new();
        let mut used_clickwheel = false;
        for &(key, route) in model.keymap {
            match route {
                KeyRoute::ClickWheel => {
                    used_clickwheel = true;
                    let master = match key {
                        Ipod4gKey::Action => controls_tx.action.clone(),
                        Ipod4gKey::Up => controls_tx.up.clone(),
                        Ipod4gKey::Down => controls_tx.down.clone(),
                        Ipod4gKey::Left => controls_tx.left.clone(),
                        Ipod4gKey::Right => controls_tx.right.clone(),
                        Ipod4gKey::Hold => continue, // Hold is wired separately
                    };
                    keys.insert(key, controls::KeySink::ClickWheel(master.clone()));
                    sys.synthetic_controls.insert(key, controls::KeySink::ClickWheel(master));
                }
                KeyRoute::Gpio(block, pin, sticky, active) => {
                    let (mut key_tx, key_rx) = gpio::new(gpio_changed.clone(), controls::key_label(key));
                    if let Some(gpio_block) = sys.devices.gpio_block(block) {
                        gpio_block.lock().unwrap().register_in(pin as usize, key_rx);
                    }
                    if active == KeyActive::Low { key_tx.set_high(); }
                    let sink = controls::KeySink::Gpio(
                        key_tx,
                        sticky == KeySticky::True,
                        active == KeyActive::High,
                    );
                    keys.insert(key, sink.clone());
                    sys.synthetic_controls.insert(key, sink);
                }
            }
        }

        // wheels
        if used_clickwheel {
            if let Some(opto) = sys.devices.opto() {
                opto.register_controls(controls_rx, hold_rx);
            }
        } else if model.alias == "1g" || model.alias == "3g" {
            let (scroll1_tx, scroll1_rx) = gpio::new(gpio_changed.clone(), "Scroll1");
            let (scroll2_tx, scroll2_rx) = gpio::new(gpio_changed.clone(), "Scroll2");
            {
                let mut gpio_abcd = sys.devices.gpio_abcd().lock().unwrap();
                gpio_abcd.register_in(6, scroll1_rx.clone());
                gpio_abcd.register_in(7, scroll2_rx.clone());
            }
            if let Some(scroll) = sys.devices.scroll() {
                scroll.register_controls(controls_rx, scroll1_tx, scroll2_tx);
            }
        } else if model.alias == "mini1g" {
            let (scroll1_tx, scroll1_rx) = gpio::new(gpio_changed.clone(), "Scroll1");
            let (scroll2_tx, scroll2_rx) = gpio::new(gpio_changed.clone(), "Scroll2");
            {
                let mut gpio_abcd = sys.devices.gpio_abcd().lock().unwrap();
                gpio_abcd.register_in(8 + 4, scroll1_rx.clone());
                gpio_abcd.register_in(8 + 5, scroll2_rx.clone());
            }
            if let Some(scroll) = sys.devices.scroll() {
                scroll.register_controls(controls_rx, scroll1_tx, scroll2_tx);
            }
        }

        sys.controls = Some(Ipod4gControls {
            controls: controls_tx,
            keys,
        });

        // firewire cable
        if model.alias == "1g" {
            let (mut charger_tx, charger_rx) = gpio::new(gpio_changed.clone(), "Charger");
            let mut gpio_abcd = sys.devices.gpio_abcd().lock().unwrap();
            gpio_abcd.register_in(2*8 + 7, charger_rx.clone());
            charger_tx.set_high();
        }

        // sandbox
        {
            let (mut charger_tx, charger_rx) = gpio::new(gpio_changed.clone(), "Sandbox");
            let mut gpio_abcd = sys.devices.gpio_abcd().lock().unwrap();
            gpio_abcd.register_in(0*8 + 6, charger_rx.clone());
            charger_tx.set_high();
        }

        // Run the HLE bootloader if an HLE boot was requested
        if let BootKind::HLEBoot { fw_file } = boot_kind {
            run_hle_bootloader(&mut sys, fw_file)?
        }

        // connect HDD only on SoCs with an IDE controller
        if let Some(eidecon) = sys.devices.eidecon_mut() {
            eidecon.as_ide().attach(devices::ide::IdeIdx::IDE0, hdd);
        }

        Ok(sys)
    }

    /// Set keys hold at boot
    pub fn set_hold_keys(&mut self, keys: impl IntoIterator<Item = Ipod4gKey>) {
        let keys = keys.into_iter().collect::<Vec<_>>();
        if !keys.is_empty() {
            self.boot_hold = Some(keys);
        }
    }

    fn warm_reset(&mut self) {
        self.devices.memcon().reset();
        self.devices.cachecon().reset();
        self.devices.evp().reset();
        self.devices.cpucon().reset();
        self.devices.intcon().reset();
        self.devices.devcon().reset();

        self.cpu = Cpu::new();
        self.cop = Cpu::new();
    }

    /// Run the system for a single CPU instruction, returning `true` if the
    /// system is still running, or `false` upon reaching some sort of "graceful
    /// exit" condition (e.g: power-off).
    fn step(
        &mut self,
        _halt_block_mode: BlockMode,
        mut sniff_memory: (&[u32], impl FnMut(CpuId, MemAccess)),
    ) -> FatalMemResult<bool> {
        if self.frozen {
            return Ok(true);
        }

        if let Some(keys) = self.boot_hold.take() {
            let mut sinks = keys
                .iter()
                .filter_map(|key| self.synthetic_controls.get(key).cloned())
                .collect::<Vec<_>>();

            self.executor
                .spawner()
                .spawn(async move {
                    for sink in sinks.iter_mut() {
                        sink.set(true);
                    }

                    Timeout::new(BOOT_HOLD_DURATION).await;

                    for sink in sinks.iter_mut() {
                        if !sink.is_sticky() {
                            sink.set(false);
                        }
                    }
                })
                .expect("failed to spawn boot-hold task");
        }

        if self
            .reset_requested
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            info!("system reset requested");
            self.warm_reset();
            return Ok(true);
        }

        // TODO: if neither CPU is running, efficiently block until the next IRQ

        let devices = &mut self.devices;
        for (cpu, cpuid) in [(&mut self.cpu, CpuId::Cpu), (&mut self.cop, CpuId::Cop)].iter_mut() {
            if !devices.cpucon().is_cpu_running(*cpuid) {
                continue;
            }

            // XXX: armv4t_emu doesn't currently expose any way to differentiate between
            // instruction-fetch reads, and regular reads. Therefore, it's impossible to
            // enforce MMU "execute" protection bits...

            // FIXME: this approach is kinda gross. Maybe add a some "ctx" to `Memory`?
            devices.set_cpuid(*cpuid);

            let mut sniffer = MemSniffer::new(devices, sniff_memory.0, |access| {
                sniff_memory.1(*cpuid, access)
            });
            let mut mem = MemoryAdapter::new(&mut sniffer);
            cpu.step(&mut mem);
            if let Some((access, e)) = mem.exception.take() {
                e.resolve(
                    "MMIO",
                    MemExceptionCtx {
                        pc: cpu.reg_get(cpu.mode(), reg::PC),
                        access,
                        in_device: format!("{}, {}", cpuid, devices.probe(access.offset)),
                    },
                )?;
            }
        }

        if self.skip_irq_check {
            return Ok(true);
        }

        // TODO: don't run this on every cycle?
        self.executor.run_until_stalled();

        // XXX: this is terrible. truly god awful. it _really_ needs to be rewritten,
        // reorganized, and moved somewhere more appropriate.
        if self.dma_pending.check() {
            self.dma_pending.clear();
            if let Some((kind, addr)) = devices.do_ide_dma() {
                use crate::memory::MemAccessKind;
                match kind {
                    MemAccessKind::Read => {
                        let val = devices
                            .eidecon_mut()
                            .unwrap()
                            .as_ide()
                            .read16(devices::ide::IdeReg::Data)
                            .unwrap();
                        devices.w16(addr, val).unwrap();
                    }
                    MemAccessKind::Write => {
                        let val = devices.r16(addr).unwrap();
                        devices
                            .eidecon_mut()
                            .unwrap()
                            .as_ide()
                            .write16(devices::ide::IdeReg::Data, val)
                            .unwrap();
                    }
                    MemAccessKind::Execute => {
                        panic!("Unsupported execute DMA");
                    }
                }
            }
        }

        // TODO?: explore adding callbacks to the signaling system
        if self.gpio_changed.check_and_clear() {
            devices.update_gpios();
        }
        if self.i2c_changed.check_and_clear() {
            if let Some(opto) = devices.opto() {
                opto.on_change();
            }
            if let Some(scroll) = devices.scroll() {
                scroll.on_change();
            }
        }

        if self.irq_pending.check() {
            use armv4t_emu::Exception;

            let (cpu_status, cop_status) = devices.intcon().interrupt_status();
            let normal_irq_vec = devices.evp().normal_irq_vec();
            let high_priority_irq_vec = devices.evp().high_priority_irq_vec();

            for (core, cpuid, status) in [
                (&mut self.cpu, CpuId::Cpu, cpu_status),
                (&mut self.cop, CpuId::Cop, cop_status),
            ]
            .iter_mut()
            {
                if status.irq {
                    devices.cpucon().wake_on_interrupt(*cpuid);
                    let taken = core.irq_enable();
                    core.exception(Exception::Interrupt);
                    if taken {
                        vector_via_evp(core, devices, normal_irq_vec);
                    }

                    if core.irq_enable() {
                        self.irq_pending.clear();
                    }
                }
                if status.fiq {
                    devices.cpucon().wake_on_interrupt(*cpuid);
                    let taken = core.fiq_enable();
                    core.exception(Exception::FastInterrupt);
                    if taken {
                        vector_via_evp(core, devices, high_priority_irq_vec);
                    }

                    if core.fiq_enable() {
                        self.irq_pending.clear();
                    }
                }
            }
        }

        Ok(true)
    }

    /// Run the system, returning successfully on "graceful exit"
    /// (e.g: power-off).
    pub fn run(&mut self) -> FatalMemResult<()> {
        let dummy_sniff_memory = |_, _| {};
        while self.step(BlockMode::Blocking, (&[], dummy_sniff_memory))? {}
        Ok(())
    }

    /// Run the system, returning successfully on "graceful exit" (e.g:
    /// power-off). This method will return after the specified number of cycles
    /// have been executed.
    pub fn run_cycles(&mut self, cycles: usize) -> FatalMemResult<()> {
        let dummy_sniff_memory = |_, _| {};
        for _ in 0..cycles {
            self.step(BlockMode::Blocking, (&[], dummy_sniff_memory))?;
        }
        Ok(())
    }

    /// Freeze the system such that `step` becomes a noop. Called prior to
    /// spawning a "post-mortem" GDB session.
    ///
    /// WARNING - THERE IS NO WAY TO "THAW" A FROZEN SYSTEM!
    pub fn freeze(&mut self) {
        self.frozen = true;
    }

    /// Return the system's RenderCallback method.
    pub fn render_callback(&self) -> RenderCallback {
        self.devices.render_callback(&self.model)
    }
}

impl Bus {
    fn new(
        task_spawner: Spawner,
        irq_pending: irq::Pending,
        dma_pending: irq::Pending,
        model: Model,
        flash_rom: Option<Box<[u8]>>,
    ) -> Bus {
        match model.soc {
            SoC::Pp5002 => Bus::Pp5002(pp::PP5002Bus::new(
                model,
                task_spawner,
                irq_pending,
                dma_pending,
                flash_rom,
            )),
            SoC::Pp5020 => Bus::Pp502x(pp::PP502xBus::new(
                model,
                task_spawner,
                irq_pending,
                dma_pending,
                flash_rom,
            )),
            SoC::Pp5022 => Bus::Pp502x(pp::PP502xBus::new(
                model,
                task_spawner,
                irq_pending,
                dma_pending,
                flash_rom,
            )),
        }
    }

    fn set_cpuid(&mut self, cpuid: CpuId) {
        match self {
            Bus::Pp5002(bus) => bus.set_cpuid(cpuid),
            Bus::Pp502x(bus) => {
                bus.set_cpuid(cpuid);
                bus.mailbox.set_cpuid(cpuid);
            }
        }
    }

    fn cpucon(&mut self) -> &mut dyn devices::CpuConDevice {
        match self {
            Bus::Pp5002(bus) => &mut bus.cpucon,
            Bus::Pp502x(bus) => &mut bus.cpucon,
        }
    }

    fn memcon(&mut self) -> &mut devices::MemCon {
        match self {
            Bus::Pp5002(bus) => &mut bus.memcon,
            Bus::Pp502x(bus) => &mut bus.memcon,
        }
    }

    fn cachecon(&mut self) -> &mut devices::CacheCon {
        match self {
            Bus::Pp5002(bus) => &mut bus.cachecon,
            Bus::Pp502x(bus) => &mut bus.cachecon,
        }
    }

    fn evp(&mut self) -> &mut devices::Evp {
        match self {
            Bus::Pp5002(bus) => &mut bus.evp,
            Bus::Pp502x(bus) => &mut bus.evp,
        }
    }

    fn intcon(&mut self) -> &mut devices::IntCon {
        match self {
            Bus::Pp5002(bus) => &mut bus.intcon,
            Bus::Pp502x(bus) => &mut bus.intcon,
        }
    }

    fn devcon(&mut self) -> &mut devices::DevCon {
        match self {
            Bus::Pp5002(bus) => &mut bus.devcon,
            Bus::Pp502x(bus) => &mut bus.devcon,
        }
    }

    pub(super) fn flash(&self) -> &devices::Flash {
        match self {
            Bus::Pp5002(bus) => &bus.flash,
            Bus::Pp502x(bus) => &bus.flash,
        }
    }

    pub(super) fn sdram(&mut self) -> &mut devices::AsanRam {
        match self {
            Bus::Pp5002(bus) => &mut bus.sdram,
            Bus::Pp502x(bus) => &mut bus.sdram,
        }
    }

    pub(super) fn fastram(&mut self) -> &mut devices::AsanRam {
        match self {
            Bus::Pp5002(bus) => &mut bus.fastram,
            Bus::Pp502x(bus) => &mut bus.fastram,
        }
    }

    pub(super) fn gpio_abcd(&mut self) -> &ArcMutexDevice<devices::GpioBlock> {
        match self {
            Bus::Pp5002(bus) => &bus.gpio_abcd,
            Bus::Pp502x(bus) => &bus.gpio_abcd,
        }
    }

    pub(super) fn gpio_block(&mut self, which: GpioBlockId) -> Option<&ArcMutexDevice<devices::GpioBlock>> {
        match (self, which) {
            (Bus::Pp5002(bus), GpioBlockId::Abcd) => Some(&bus.gpio_abcd),
            (Bus::Pp5002(_), _) => None,
            (Bus::Pp502x(bus), GpioBlockId::Abcd) => Some(&bus.gpio_abcd),
            (Bus::Pp502x(bus), GpioBlockId::Efgh) => Some(&bus.gpio_efgh),
            (Bus::Pp502x(bus), GpioBlockId::Ijkl) => Some(&bus.gpio_ijkl),
        }
    }

    fn scroll(&mut self) -> Option<&mut devices::ScrollWheel> {
        match self {
            Bus::Pp5002(bus) => Some(&mut bus.scroll),
            Bus::Pp502x(bus) => Some(&mut bus.scroll),
        }
    }
    fn opto(&mut self) -> Option<&mut devices::OptoWheel> {
        match self {
            Bus::Pp5002(_) => None,
            Bus::Pp502x(bus) => Some(&mut bus.opto),
        }
    }

    fn update_gpios(&mut self) {
        match self {
            Bus::Pp5002(bus) => {
                bus.gpio_abcd.lock().unwrap().update();
            }
            Bus::Pp502x(bus) => {
                bus.gpio_abcd.lock().unwrap().update();
                bus.gpio_efgh.lock().unwrap().update();
                bus.gpio_ijkl.lock().unwrap().update();
            }
        }
    }

    fn do_ide_dma(&mut self) -> Option<(crate::memory::MemAccessKind, u32)> {
        match self {
            Bus::Pp5002(_) => None,
            Bus::Pp502x(bus) => {
                if bus.dmacon0.do_ide_dma() {
                    bus.eidecon.do_dma().ok()
                } else {
                    None
                }
            }
        }
    }

    fn eidecon_mut(&mut self) -> Option<&mut devices::EIDECon> {
        match self {
            Bus::Pp5002(bus) => Some(&mut bus.eidecon),
            Bus::Pp502x(bus) => Some(&mut bus.eidecon),
        }
    }

    fn render_callback(&self, model: &Model) -> RenderCallback {
        match self {
            Bus::Pp5002(bus) => bus.render_callback(model),
            Bus::Pp502x(bus) => bus.render_callback(model),
        }
    }
}

impl Device for Bus {
    fn kind(&self) -> &'static str {
        match self {
            Bus::Pp5002(_) => "Pp5002Bus",
            Bus::Pp502x(_) => "Pp502xBus",
        }
    }

    fn probe(&self, addr: u32) -> Probe {
        match self {
            Bus::Pp5002(bus) => bus.probe(addr),
            Bus::Pp502x(bus) => bus.probe(addr),
        }
    }
}

impl Memory for Bus {
    fn r8(&mut self, addr: u32) -> MemResult<u8> {
        match self {
            Bus::Pp5002(bus) => bus.r8(addr),
            Bus::Pp502x(bus) => bus.r8(addr),
        }
    }

    fn r16(&mut self, addr: u32) -> MemResult<u16> {
        match self {
            Bus::Pp5002(bus) => bus.r16(addr),
            Bus::Pp502x(bus) => bus.r16(addr),
        }
    }

    fn r32(&mut self, addr: u32) -> MemResult<u32> {
        match self {
            Bus::Pp5002(bus) => bus.r32(addr),
            Bus::Pp502x(bus) => bus.r32(addr),
        }
    }

    fn w8(&mut self, addr: u32, val: u8) -> MemResult<()> {
        match self {
            Bus::Pp5002(bus) => bus.w8(addr, val),
            Bus::Pp502x(bus) => bus.w8(addr, val),
        }
    }

    fn w16(&mut self, addr: u32, val: u16) -> MemResult<()> {
        match self {
            Bus::Pp5002(bus) => bus.w16(addr, val),
            Bus::Pp502x(bus) => bus.w16(addr, val),
        }
    }

    fn w32(&mut self, addr: u32, val: u32) -> MemResult<()> {
        match self {
            Bus::Pp5002(bus) => bus.w32(addr, val),
            Bus::Pp502x(bus) => bus.w32(addr, val),
        }
    }

    fn x16(&mut self, addr: u32) -> MemResult<u16> {
        match self {
            Bus::Pp5002(bus) => bus.x16(addr),
            Bus::Pp502x(bus) => bus.x16(addr),
        }
    }

    fn x32(&mut self, addr: u32) -> MemResult<u32> {
        match self {
            Bus::Pp5002(bus) => bus.x32(addr),
            Bus::Pp502x(bus) => bus.x32(addr),
        }
    }
}
