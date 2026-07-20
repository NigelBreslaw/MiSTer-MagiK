// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::model::{Operation, Risk};

#[must_use]
pub fn operation(id: &str) -> Option<Operation> {
    match id {
        "repo.diff-check" => Some(Operation {
            id: "repo.diff-check".into(),
            title: "Check patch whitespace".into(),
            risk: Risk::ReadOnly,
            program: "git".into(),
            args: vec!["diff".into(), "--check".into()],
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_identity_is_stable() {
        let left = operation("repo.diff-check").unwrap();
        let right = operation("repo.diff-check").unwrap();
        assert_eq!(left, right);
        assert_eq!(left.id, "repo.diff-check");
    }
}
