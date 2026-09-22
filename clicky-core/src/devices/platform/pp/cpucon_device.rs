use crate::devices::platform::pp::common::CpuId;

pub trait CpuConDevice: std::fmt::Debug {
    fn reset(&mut self);
    fn is_cpu_running(&mut self, cpu: CpuId) -> bool;
    fn wake_on_interrupt(&mut self, cpu: CpuId);
}
