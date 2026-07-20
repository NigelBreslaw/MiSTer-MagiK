// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::model::{Operation, Risk};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rejection {
    pub operation_id: String,
    pub reason: &'static str,
}

pub fn authorize(operation: &Operation, maximum: Risk) -> Result<(), Rejection> {
    if operation.risk > maximum {
        Err(Rejection {
            operation_id: operation.id.clone(),
            reason: "operation exceeds the authorized risk tier",
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_operations_above_the_allowed_risk() {
        let operation = Operation {
            id: "device.deploy".into(),
            title: "Deploy".into(),
            risk: Risk::DeviceWrite,
            program: "scripts/deploy-rust.sh".into(),
            args: vec![],
            reason: "deployment requested".into(),
            failure_hint: "inspect the recorded run".into(),
        };
        assert!(authorize(&operation, Risk::LocalWrite).is_err());
    }
}
