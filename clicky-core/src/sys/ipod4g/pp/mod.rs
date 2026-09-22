//! SoC-specific memory maps and bus implementations for the iPod system.

mod core;
pub mod pp5002;
pub mod pp502x;

pub use core::PpCore;
pub use pp5002::PP5002Bus;
pub use pp502x::PP502xBus;
