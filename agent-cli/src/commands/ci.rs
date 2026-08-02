// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::AgentResult;

pub fn run_local_host(args: Vec<String>) -> AgentResult<()> {
    crate::host::run_cli_args(args).map_err(|error| error.to_string().into())
}
