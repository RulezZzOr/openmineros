pub mod bitbang;
pub mod fpga;
pub mod i2c;
#[cfg(unix)]
pub mod mmio;
#[cfg(unix)]
pub mod uboot_env;
#[cfg(unix)]
pub mod uio;
pub mod x19;
pub mod x19_board;
pub mod x19_mine;
pub mod x19_work;

use openmineros_common::{BoardFamily, BoardProfile, Capability, CapabilitySet};

pub fn profile() -> BoardProfile {
    BoardProfile {
        family: BoardFamily::Xilinx,
        soc: "Zynq",
        recovery: "external microSD",
        capabilities: CapabilitySet::from_flags([
            Capability::InstallSd,
            Capability::InstallCommander,
            Capability::UpdateAb,
            Capability::FanControl,
            Capability::ChainIsolation,
            Capability::SafeMode,
        ]),
    }
}
