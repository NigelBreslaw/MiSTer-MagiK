// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::{AgentError, AgentResult};

pub fn run_phases<A: ?Sized, P: Copy>(
    actions: &mut A,
    phases: &[(P, u8)],
    progress: &mut dyn FnMut(P, u8) -> AgentResult<()>,
    mut run: impl FnMut(&mut A, P) -> AgentResult<()>,
    label: impl Fn(P) -> &'static str,
) -> AgentResult<()> {
    for &(phase, percent) in phases {
        progress(phase, percent).map_err(AgentError::cancelled)?;
        run(actions, phase).map_err(|error| AgentError::phase(label(phase), error))?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_restorable_phases<A: ?Sized, P: Copy>(
    actions: &mut A,
    phases: &[(P, u8)],
    progress: &mut dyn FnMut(P, u8) -> AgentResult<()>,
    mut run: impl FnMut(&mut A, P) -> AgentResult<()>,
    mut restore: impl FnMut(&mut A) -> AgentResult<()>,
    needs_restore: impl Fn(&A) -> bool,
    is_restore: impl Fn(P) -> bool,
    label: impl Fn(P) -> &'static str,
    recovery_context: &'static str,
) -> AgentResult<()> {
    for &(phase, percent) in phases {
        if let Err(error) = progress(phase, percent) {
            return restore_workflow(
                actions,
                AgentError::cancelled(error),
                &mut restore,
                &needs_restore,
                recovery_context,
            );
        }
        let result = if is_restore(phase) {
            restore(actions)
        } else {
            run(actions, phase)
        };
        if let Err(error) = result {
            return restore_workflow(
                actions,
                AgentError::phase(label(phase), error),
                &mut restore,
                &needs_restore,
                recovery_context,
            );
        }
    }
    Ok(())
}

fn restore_workflow<A: ?Sized>(
    actions: &mut A,
    error: AgentError,
    restore: &mut impl FnMut(&mut A) -> AgentResult<()>,
    needs_restore: &impl Fn(&A) -> bool,
    recovery_context: &str,
) -> AgentResult<()> {
    if !needs_restore(actions) {
        return Err(error);
    }
    match restore(actions) {
        Ok(()) => Err(format!("{error}; restore=complete").into()),
        Err(restore_error) => Err(AgentError::recovery_required(
            error.to_string(),
            format!("{recovery_context} restore failed ({restore_error})"),
        )),
    }
}
