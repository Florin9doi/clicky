#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
#![allow(unused_mut)]
#![allow(unused_assignments)]

use crate::devices::prelude::*;

mod reg {
    pub const VERSION:          u32 = 0x00;
    pub const ACK:              u32 = 0x04;
    pub const CTRL:             u32 = 0x08;
    pub const INTERRUPT:        u32 = 0x0C;
    pub const INT_MASK:         u32 = 0x10;
    pub const CYCLE_TIMER:      u32 = 0x14;
    pub const DIAGNOSTIC:       u32 = 0x18;
    pub const RESERVED:         u32 = 0x1c;
    pub const PHYACCESS:        u32 = 0x20;
    pub const BUS_RESET:        u32 = 0x24;
    pub const TIMELIMIT:        u32 = 0x28;
    pub const ATF_STATUS:       u32 = 0x2C;
    pub const ARF_STATUS:       u32 = 0x30;
    pub const MTQ_STATUS:       u32 = 0x34;
    pub const MRF_STATUS:       u32 = 0x38;
    pub const CTQ_STATUS:       u32 = 0x3C;
    pub const CRF_STATUS:       u32 = 0x40;
    pub const ORB_FETCH_CTRL:   u32 = 0x44;
    pub const MANAGEMENT_AGENT: u32 = 0x48;
    pub const COMMAND_AGENT:    u32 = 0x4C;
    pub const AGENT_CTRL:       u32 = 0x50;
    pub const ORB_PTR1:         u32 = 0x54;
    pub const ORB_PTR2:         u32 = 0x58;
    pub const AGENT_STATUS:     u32 = 0x5C;
    pub const TXTIMER_CTRL:     u32 = 0x60;
    pub const TXTIMER_STATUS1:  u32 = 0x64;
    pub const TXTIMER_STATUS2:  u32 = 0x68;
    pub const TXTIMER_STATUS3:  u32 = 0x6C;
    pub const WRITE_FIRST:      u32 = 0x70;
    pub const WRITE_CONTINUE:   u32 = 0x74;
    pub const WRITE_UPDATE:     u32 = 0x78;
    pub const ARF_DATA:         u32 = 0x80;
    pub const MRF_DATA:         u32 = 0x84;
    pub const CRF_DATA:         u32 = 0x88;
    pub const CFR_CTRL:         u32 = 0x8C;
    pub const DMA_CTRL:         u32 = 0x90;
    pub const BI_CTRL:          u32 = 0x94;
    pub const DXF_SIZE:         u32 = 0x98;
    pub const DXF_AVAIL:        u32 = 0x9C;
    pub const DXF_ACK:          u32 = 0xA0;
    pub const DTF_1ST_CONTINUE: u32 = 0xA4;
    pub const DTF_UPDATE:       u32 = 0xA8;
    pub const DRF_DATA:         u32 = 0xAC;
    pub const DTF_CTRL0:        u32 = 0xB0;
    pub const DTF_CTRL1:        u32 = 0xB4;
    pub const DTF_CTRL2:        u32 = 0xB8;
    pub const DTF_CTRL3:        u32 = 0xBC;
    pub const DRF_CTRL0:        u32 = 0xC0;
    pub const DRF_CTRL1:        u32 = 0xC4;
    pub const DRF_CTRL2:        u32 = 0xC8;
    pub const DRF_CTRL3:        u32 = 0xCC;
    pub const DRF_HDR0:         u32 = 0xD0;
    pub const DRF_HDR1:         u32 = 0xD4;
    pub const DRF_HDR2:         u32 = 0xD8;
    pub const DRF_HDR3:         u32 = 0xDC;
    pub const DRF_TAILER:       u32 = 0xE0;
    pub const DXF_EXPCTD_VALUE: u32 = 0xE4;
    pub const DXF_HDRSTAT0:     u32 = 0xE8;
    pub const DXF_HDRSTAT1:     u32 = 0xEC;
    pub const DXF_HDRSTAT2:     u32 = 0xF0;
    pub const DXF_HDRSTAT3:     u32 = 0xF4;
    pub const LOG_ROM_CTRL:     u32 = 0xF8;
    pub const LOG_ROM_DATA:     u32 = 0xFC;
}

mod phy {
    type Range = std::ops::RangeInclusive<usize>;
    pub const RD_PY       : usize = 31;
    pub const WR_PY       : usize = 30;
    pub const PHY_RG_AD   : Range = 24 ..= 27;
    pub const PHY_RG_DATA : Range = 16 ..= 23;
    pub const PHY_RX_AD   : Range =  8 ..= 11;
    pub const PHY_RX_DATA : Range =  0 ..=  7;
}

// TSB43AA82 reg | 00h         | 04h         | 08h         | 0ch         | ...
// mapped to     | 00 02 04 06 | 08 0a 0c 0e | 10 12 14 16 | 18 1a 1c 1e |

#[derive(Debug)]
pub struct TSB43AA82 {
    rreg: u32,
    rval: u32,
    wreg: u32,
    wval: u32,
    int_event: u32,
    int_mask: u32,
    phyaccess: u32,
    cycle_timer: u32,

    hc_control: u32,
    link_control: u32,
    self_id_buffer: u32,
    reg: Box<[u32; 0x80]>,
}

impl TSB43AA82 {
    pub fn new() -> TSB43AA82 {
        TSB43AA82 {
            rreg: 0,
            rval: 0,
            wreg: 0,
            wval: 0,
            int_event: 0,
            int_mask: 0,
            phyaccess: 0,
            cycle_timer: 0,

            hc_control: 0,
            link_control: 0,
            self_id_buffer: 0,
            reg: Box::new([0; 0x80]),
        }
    }

    fn read_phy(&self, _addr: u8) -> u8 {
        0
    }

    fn read_reg(&mut self, offset: u32) -> u32 {
        match offset {
            reg::VERSION => 0x4300_8203,
            reg::INTERRUPT => self.int_event | 0x8000_0000,
            reg::INT_MASK => self.int_mask,
            reg::PHYACCESS => self.phyaccess,
            reg::CYCLE_TIMER => { self.cycle_timer += 1; self.cycle_timer }
            reg::DMA_CTRL => 0,
            reg::BI_CTRL => 0,
            reg::DRF_CTRL0 => 0,
            reg::DTF_CTRL0 => 0,
            reg::LOG_ROM_CTRL => 0,
            _ => 0xDEADCAFE,
        }
    }
    fn write_reg(&mut self, offset: u32, val: u32) {
        match offset {
            reg::INTERRUPT => self.int_event &= !val,
            reg::INT_MASK => self.int_mask = val,
            reg::PHYACCESS => {
                self.phyaccess = val & !(1 << phy::RD_PY | 1 << phy::WR_PY);
                let phy_rg_ad = val.get_bits(phy::PHY_RG_AD);
                let phy_rg_data = val.get_bits(phy::PHY_RG_DATA);
                // write
                if val.get_bit(phy::WR_PY) {
                    {debug!(target: "FW", "  PHY write : reg:{:8x} val:{:8x}", phy_rg_ad, phy_rg_data);}
                    if phy_rg_ad == 1 && (phy_rg_data & 0x40) == 0x40 {
                        {debug!(target: "FW", "Phy1 - IBR=1 - Initiate bus reset");}
                    }
                }
                // read
                if val.get_bit(phy::RD_PY) {
                    let data = match phy_rg_ad {
                        1 => 0x3f, // phy reg 1, RHB(0x80)=0, IBR(0x40)=0, Gap_Count=0x3f
                        _ => 0,
                    };
                    self.phyaccess.set_bits(phy::PHY_RX_AD, phy_rg_ad);
                    self.phyaccess.set_bits(phy::PHY_RX_DATA, data);
                    {debug!(target: "FW", "  PHY read  : reg:{:8x} val:{:8x} phyacc:{:x}", phy_rg_ad, data, self.phyaccess);}
                    self.int_event |= 0xffff_0000;
                }
            }
            _ => {},
        }
    }
}

impl Device for TSB43AA82 {
    fn kind(&self) -> &'static str {
        "Firewire (TI)"
    }

    fn probe(&self, offset: u32) -> Probe {
        let reg = match offset >> 1 & !0x3  {
            reg::VERSION          => "00h - Version/Revision",
            reg::CTRL             => "08h - Control",
            reg::INTERRUPT        => "0Ch - Interrupt",
            reg::INT_MASK         => "10h - Interrupt Mask",
            reg::CYCLE_TIMER      => "14h - Cycle Timer",
            reg::PHYACCESS        => "20h - PHY Access",
            reg::TIMELIMIT        => "28h - Time Limit",
            reg::ATF_STATUS       => "2Ch - ATF Status",
            reg::ARF_STATUS       => "30h - ARF Status",
            reg::MTQ_STATUS       => "34h - MTQ Status",
            reg::MRF_STATUS       => "38h - MRF Status",
            reg::CTQ_STATUS       => "3Ch - CTQ Status",
            reg::CRF_STATUS       => "40h - CRF Status",
            reg::ORB_FETCH_CTRL   => "44h - ORB Fetch Control",
            reg::MANAGEMENT_AGENT => "48h - Management Agent",
            reg::COMMAND_AGENT    => "4Ch - Command Agent",
            reg::WRITE_FIRST      => "70h - Write-First",
            reg::CFR_CTRL         => "8Ch - CFR Control",
            reg::DMA_CTRL         => "90h - DMA Control",
            reg::BI_CTRL          => "94h - Bulky If Control",
            reg::DXF_SIZE         => "98h - DxF Size",
            reg::DTF_CTRL0        => "B0h - DTF Control 0",
            reg::DRF_CTRL0        => "C0h - DRF Control 0",
            reg::LOG_ROM_CTRL     => "F8h - Log/ROM Control",
            reg::LOG_ROM_DATA     => "FCh - Log ROM Data",
            other => Box::leak(format!("{:02X}h - Unknown (?)", other).into_boxed_str()),
        };

        Probe::Register(reg)
    }
}

impl Memory for TSB43AA82 {
    fn r16(&mut self, offset: u32) -> MemResult<u16> {
        let fw_reg = (offset >> 1) & !0x3;
        let step   = (offset >> 1) & 0x3;
        match step {
            0 => {
                self.rreg = fw_reg;
                self.rval = self.read_reg(fw_reg);
                if true
                //  && fw_reg != 0x0c // INTERRUPT
                //  && fw_reg != 0x10 // INT_MASK
                 && fw_reg != 0x14 // CYCLE_TIMER
                 {debug!(target: "FW", " read reg:{:8x} val:{:8x} ({})", self.rreg, self.rval, self.probe(offset));}
                // if self.rval == 0xDEADCAFE { return Err(Unexpected) }
                Err(StubRead(Trace, (self.rval) & 0xff as u32))
            }
            1..=3 => {
                if self.rreg != fw_reg { return Err(Unexpected) }
                Err(StubRead(Trace, (self.rval >> (step * 8)) & 0xff))
            }
            _ => { return Err(Unexpected) }
        }
    }

    fn w16(&mut self, offset: u32, val: u16) -> MemResult<()> {
        let fw_reg = (offset >> 1) & !0x3;
        let step   = (offset >> 1) & 0x3;
        match step {
            0 => {
                self.wreg = fw_reg;
                self.wval = val as u32;
            }
            1..=2 => {
                if self.wreg != fw_reg { return Err(Unexpected) }
                self.wval |= (val as u32) << (step * 8);
            }
            3 => {
                if self.wreg != fw_reg { return Err(Unexpected) }
                self.wval |= (val as u32) << 24;
                if true
                //  && fw_reg != 0x0c // INTERRUPT
                //  && fw_reg != 0x10 // INT_MASK
                {debug!(target: "FW", "write reg:{:8x} val:{:8x} ({})", self.wreg, self.wval, self.probe(offset));}
                self.write_reg(self.wreg, self.wval);
            }
            _ => { return Err(Unexpected) }
        }
        return Ok(())
    }

    fn r32(&mut self, offset: u32) -> MemResult<u32> {
        Err(Unexpected)
    }

    fn w32(&mut self, offset: u32, val: u32) -> MemResult<()> {
        Err(Unexpected)
    }
}
