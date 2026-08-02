// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Component {
    Repository,
    AgentCli,
    FramebufferLab,
    MisterApp,
    Desktop,
    Catalog,
    CoreCrate,
    Runtime,
    PlatformContracts,
    Kernel,
    Fpga,
    DeviceAgent,
    Manager,
    Scripts,
    Workflow,
    Documentation,
    Private,
    Tools,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentImpact {
    None,
    Runtime,
    Platform,
}

impl Component {
    #[must_use]
    pub const fn deployment_impact(self) -> DeploymentImpact {
        match self {
            Self::MisterApp
            | Self::Catalog
            | Self::CoreCrate
            | Self::Runtime
            | Self::DeviceAgent
            | Self::Manager => DeploymentImpact::Runtime,
            Self::PlatformContracts | Self::Kernel | Self::Fpga => DeploymentImpact::Platform,
            _ => DeploymentImpact::None,
        }
    }
}

impl DeploymentImpact {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Runtime => "runtime",
            Self::Platform => "platform",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "none" => Ok(Self::None),
            "runtime" => Ok(Self::Runtime),
            "platform" => Ok(Self::Platform),
            _ => Err(format!("invalid persisted deployment impact: {value}")),
        }
    }
}

pub fn classify(path: &Path) -> Option<Component> {
    if matches!(path.to_str(), Some("Cargo.toml" | "Cargo.lock")) {
        Some(Component::PlatformContracts)
    } else if path
        .parent()
        .is_some_and(|parent| parent.as_os_str().is_empty())
        || path.file_name().and_then(|name| name.to_str()) == Some("AGENTS.md")
        || path.starts_with("LICENSES")
        || path.starts_with(".codex")
        || is_repository_dot_config(path)
    {
        Some(Component::Repository)
    } else if matches!(
        path.to_str(),
        Some(
            "apps/mister/Dockerfile.cross-armv7"
                | "apps/mister/rust-toolchain.toml"
                | "apps/mister/Cross.toml"
                | "scripts/build-scanout-slots-module.sh"
        )
    ) {
        Some(Component::PlatformContracts)
    } else if path.starts_with("agent-cli") || is_retired_host_package(path) {
        Some(Component::AgentCli)
    } else if path.starts_with("apps/framebuffer-lab")
        || path.starts_with("apps/startup-particle-lab")
    {
        Some(Component::FramebufferLab)
    } else if path.starts_with("apps/mister") {
        Some(Component::MisterApp)
    } else if path.starts_with("apps/desktop") {
        Some(Component::Desktop)
    } else if path.starts_with("crates/mister-ini") {
        Some(Component::Manager)
    } else if path.starts_with("crates/catalog") {
        Some(Component::Catalog)
    } else if path.starts_with("crates/") {
        Some(Component::CoreCrate)
    } else if path.starts_with("mister/platform/runtime") {
        Some(Component::Runtime)
    } else if path.starts_with("mister/platform/contracts") {
        Some(Component::PlatformContracts)
    } else if path.starts_with("mister/platform/kernel") {
        Some(Component::Kernel)
    } else if path.starts_with("mister/platform/fpga") {
        Some(Component::Fpga)
    } else if path.starts_with("mister/tools/agent") {
        Some(Component::DeviceAgent)
    } else if path.starts_with("mister/tools/manager") {
        Some(Component::Manager)
    } else if path.starts_with("scripts") {
        Some(Component::Scripts)
    } else if path.starts_with(".github") || path.starts_with(".githooks") {
        Some(Component::Workflow)
    } else if path.starts_with("docs") || path.starts_with("documentation") {
        Some(Component::Documentation)
    } else if path.starts_with("history") || path.starts_with("private") {
        Some(Component::Private)
    } else if path.starts_with("tools") {
        Some(Component::Tools)
    } else {
        None
    }
}

fn is_retired_host_package(path: &Path) -> bool {
    let mut components = path.iter();
    components.next().and_then(|part| part.to_str()) == Some("mister")
        && components.next().and_then(|part| part.to_str()) == Some("tools")
        && components.next().and_then(|part| part.to_str()) == Some("host")
}

pub(crate) fn is_repository_dot_config(path: &Path) -> bool {
    path.iter()
        .next()
        .and_then(|component| component.to_str())
        .is_some_and(|component| {
            component.starts_with('.') && !matches!(component, ".github" | ".githooks")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deployment_impact_is_owned_by_components() {
        assert_eq!(
            classify(Path::new("apps/mister/src/launcher.rs"))
                .unwrap()
                .deployment_impact(),
            DeploymentImpact::Runtime
        );
        assert_eq!(
            classify(Path::new("mister/platform/fpga/menu.sv"))
                .unwrap()
                .deployment_impact(),
            DeploymentImpact::Platform
        );
        assert_eq!(
            classify(Path::new("docs/device.md"))
                .unwrap()
                .deployment_impact(),
            DeploymentImpact::None
        );
        assert_eq!(
            classify(Path::new("apps/framebuffer-lab/src/main.rs")),
            Some(Component::FramebufferLab)
        );
        assert_eq!(
            classify(Path::new("apps/startup-particle-lab/src/main.rs")),
            Some(Component::FramebufferLab)
        );
        assert_eq!(
            Component::FramebufferLab.deployment_impact(),
            DeploymentImpact::None
        );
        assert!(classify(Path::new("unknown/new.tree")).is_none());
        assert_eq!(
            classify(Path::new("mister/tools/agent/src/main.rs"))
                .unwrap()
                .deployment_impact(),
            DeploymentImpact::Runtime
        );
        assert_eq!(
            classify(Path::new("apps/mister/Dockerfile.cross-armv7"))
                .unwrap()
                .deployment_impact(),
            DeploymentImpact::Platform
        );
        assert_eq!(
            classify(Path::new("scripts/build-scanout-slots-module.sh"))
                .unwrap()
                .deployment_impact(),
            DeploymentImpact::Platform
        );
        assert_eq!(
            classify(Path::new("Cargo.lock"))
                .unwrap()
                .deployment_impact(),
            DeploymentImpact::Platform
        );
        assert_eq!(
            classify(Path::new("crates/mister-ini/src/lib.rs")),
            Some(Component::Manager)
        );
        assert_eq!(
            classify(Path::new(".obsolete/config.toml")),
            Some(Component::Repository)
        );
        assert_eq!(
            classify(Path::new(".github/workflows/check.yml")),
            Some(Component::Workflow)
        );
    }
}
