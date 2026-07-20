// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Operation {
    Discover,
    Status,
    VerifyManifest,
    DeployRuntime,
    ActivateDevelopment,
    RebootWait,
    VerifyHealth,
}

impl Operation {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Discover => "discover",
            Self::Status => "status",
            Self::VerifyManifest => "verify-manifest",
            Self::DeployRuntime => "deploy-runtime",
            Self::ActivateDevelopment => "activate-development",
            Self::RebootWait => "reboot-wait",
            Self::VerifyHealth => "verify-health",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    pub operation: Operation,
    pub args: Vec<String>,
    pub deadline: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Response {
    pub operation: Operation,
    pub stdout: String,
    pub stderr: String,
    pub elapsed_ms: u128,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Failure {
    Timeout,
    Disconnected,
    ChecksumMismatch,
    Unhealthy,
    RollbackFailed,
    CommandFailed { code: Option<i32>, detail: String },
}

pub trait DeviceTransport {
    fn execute(&mut self, request: &Request) -> Result<Response, Failure>;
}

#[derive(Clone, Debug)]
pub struct HostCliTransport {
    binary: PathBuf,
    environment: BTreeMap<String, String>,
}

impl HostCliTransport {
    #[must_use]
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            environment: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_environment(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.insert(name.into(), value.into());
        self
    }

    fn command(&self, request: &Request) -> Result<Command, Failure> {
        let mut command = Command::new(&self.binary);
        command
            .args(command_args(request)?)
            .envs(&self.environment)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(command)
    }
}

impl DeviceTransport for HostCliTransport {
    fn execute(&mut self, request: &Request) -> Result<Response, Failure> {
        let started = Instant::now();
        let mut child = self
            .command(request)?
            .spawn()
            .map_err(|error| Failure::CommandFailed {
                code: None,
                detail: error.to_string(),
            })?;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    let output =
                        child
                            .wait_with_output()
                            .map_err(|error| Failure::CommandFailed {
                                code: None,
                                detail: error.to_string(),
                            })?;
                    if !output.status.success() {
                        return Err(Failure::CommandFailed {
                            code: output.status.code(),
                            detail: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                        });
                    }
                    return Ok(Response {
                        operation: request.operation,
                        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                        elapsed_ms: started.elapsed().as_millis(),
                    });
                }
                Ok(None) if started.elapsed() < request.deadline => {
                    thread::sleep(Duration::from_millis(25));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Failure::Timeout);
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Failure::CommandFailed {
                        code: None,
                        detail: error.to_string(),
                    });
                }
            }
        }
    }
}

fn command_args(request: &Request) -> Result<Vec<String>, Failure> {
    let args = match request.operation {
        Operation::Discover => vec!["connected".into()],
        Operation::Status | Operation::VerifyHealth => vec!["status".into(), "--json".into()],
        Operation::DeployRuntime => {
            if request.args.len() != 2 {
                return Err(invalid_args(request.operation));
            }
            vec![
                "agent".into(),
                "deploy-magik-bin".into(),
                request.args[0].clone(),
                request.args[1].clone(),
                "--json".into(),
            ]
        }
        Operation::ActivateDevelopment => {
            vec!["ini-select-main".into(), "MiSTer_MagiKDev".into()]
        }
        Operation::RebootWait => vec!["reboot-wait".into()],
        Operation::VerifyManifest => {
            return Err(Failure::CommandFailed {
                code: None,
                detail: "manifest verification requires a typed host operation".into(),
            });
        }
    };
    Ok(args)
}

fn invalid_args(operation: Operation) -> Failure {
    Failure::CommandFailed {
        code: None,
        detail: format!("invalid arguments for {}", operation.label()),
    }
}

#[derive(Clone, Debug, Default)]
pub struct FakeTransport {
    responses: VecDeque<Result<Response, Failure>>,
    requests: Vec<Request>,
}

impl FakeTransport {
    #[must_use]
    pub fn with_results(results: impl IntoIterator<Item = Result<Response, Failure>>) -> Self {
        Self {
            responses: results.into_iter().collect(),
            requests: Vec::new(),
        }
    }

    #[must_use]
    pub fn requests(&self) -> &[Request] {
        &self.requests
    }
}

impl DeviceTransport for FakeTransport {
    fn execute(&mut self, request: &Request) -> Result<Response, Failure> {
        self.requests.push(request.clone());
        self.responses
            .pop_front()
            .unwrap_or(Err(Failure::Disconnected))
    }
}

#[must_use]
pub fn runtime_deploy_request(local: &Path, remote: &str, deadline: Duration) -> Request {
    Request {
        operation: Operation::DeployRuntime,
        args: vec![local.display().to_string(), remote.to_owned()],
        deadline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_request_maps_to_resident_agent_command() {
        let request = runtime_deploy_request(
            Path::new("build/mister-magik-fb"),
            "/media/fat/mister-magik-dev/mister-magik-fb",
            Duration::from_secs(30),
        );
        assert_eq!(
            command_args(&request).unwrap(),
            vec![
                "agent",
                "deploy-magik-bin",
                "build/mister-magik-fb",
                "/media/fat/mister-magik-dev/mister-magik-fb",
                "--json"
            ]
        );
    }

    #[test]
    fn fake_transport_records_requests_and_injects_failures() {
        let mut transport = FakeTransport::with_results([Err(Failure::ChecksumMismatch)]);
        let request = Request {
            operation: Operation::VerifyManifest,
            args: Vec::new(),
            deadline: Duration::from_secs(1),
        };
        assert_eq!(transport.execute(&request), Err(Failure::ChecksumMismatch));
        assert_eq!(transport.requests(), &[request]);
    }

    #[test]
    fn host_process_is_killed_at_the_total_deadline() {
        let mut transport = HostCliTransport::new("/usr/bin/yes");
        let request = Request {
            operation: Operation::Discover,
            args: Vec::new(),
            deadline: Duration::from_millis(20),
        };
        assert_eq!(transport.execute(&request), Err(Failure::Timeout));
    }
}
