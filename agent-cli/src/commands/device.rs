// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::AgentResult;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum DeviceCommand {
    ArmingStatus,
    Crt {
        #[command(subcommand)]
        command: CrtCommand,
    },
    Capture {
        #[command(subcommand)]
        command: CaptureCommand,
    },
    Events,
    Fpga {
        #[command(subcommand)]
        command: DeviceFpgaCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum CrtCommand {
    Qualify(CrtQualifyArgs),
    Probe(CrtProbeArgs),
    Restore(AttendedArgs),
}

#[derive(Debug, Args)]
pub struct CrtQualifyArgs {
    #[arg(long, required = true)]
    attended: bool,
    #[arg(long)]
    pub(crate) out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct CrtProbeArgs {
    #[arg(long, required = true)]
    attended: bool,
    #[arg(long)]
    pub(crate) pattern: String,
    #[arg(long)]
    pub(crate) seconds: u64,
    #[arg(long)]
    pub(crate) out: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum CaptureCommand {
    Framebuffer(FramebufferArgs),
}

#[derive(Debug, Args)]
pub struct FramebufferArgs {
    #[arg(long, value_name = "STEM")]
    pub(crate) output: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct AttendedArgs {
    #[arg(long, required = true)]
    attended: bool,
}

#[derive(Debug, Subcommand)]
pub enum DeviceFpgaCommand {
    InstallExperimentalAgent(ExperimentalAgentArgs),
}

#[derive(Debug, Args)]
pub struct ExperimentalAgentArgs {
    #[arg(long)]
    pub(crate) agent: PathBuf,
    #[arg(long)]
    pub(crate) expected_rbf_sha256: String,
    #[arg(long, required = true)]
    attended: bool,
}

pub fn run(command: DeviceCommand) -> AgentResult<()> {
    let mutation = command.is_mutation();
    let mut device = crate::device::DeviceClient::default();
    if mutation {
        device.mutate(|device| device.run_operator(&command))
    } else {
        device.read(|device| device.run_operator(&command))
    }
}

impl DeviceCommand {
    pub(crate) fn is_mutation(&self) -> bool {
        matches!(self, Self::Crt { .. } | Self::Fpga { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: DeviceCommand,
    }
    #[test]
    fn migrated_device_commands_are_rejected() {
        for command in [
            "status",
            "mode",
            "display",
            "launcher",
            "catalog",
            "media",
            "reboot",
            "logs",
            "diagnostics",
        ] {
            assert!(
                TestCli::try_parse_from(["agent-cli", command]).is_err(),
                "{command}"
            );
        }
    }
    #[test]
    fn separate_hardware_operations_remain_explicit() {
        assert!(TestCli::try_parse_from(["agent-cli", "arming-status"]).is_ok());
        assert!(TestCli::try_parse_from(["agent-cli", "capture", "framebuffer"]).is_ok());
        assert!(TestCli::try_parse_from(["agent-cli", "crt", "qualify"]).is_err());
        assert!(TestCli::try_parse_from(["agent-cli", "crt", "qualify", "--attended"]).is_ok());
    }
}
