// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_perf_events::{
    CounterDelta, CounterGroup, PmuFailure, PmuOpenDiagnostics, event_metadata,
};
use serde_json::{Value, json};

const PROBE_ITERATIONS: usize = 1_000_000;
const PROBE_WORDS: usize = 16 * 1024;

pub fn run() {
    let summary = probe();
    crate::ui_logln!("{summary}");
    if summary.get("status").and_then(Value::as_str) != Some("ok") {
        std::process::exit(1);
    }
}

pub(crate) fn probe() -> Value {
    let (group, diagnostics) = CounterGroup::open_with_diagnostics();
    match measure_probe(group) {
        Ok((delta, checksum, read_format, scope)) if valid_probe(delta) => json!({
            "schema": "mister-magik-pmu-probe-v1",
            "status": "ok",
            "target": {
                "architecture": std::env::consts::ARCH,
                "operating_system": std::env::consts::OS,
                "scope": "calling-thread-user-space",
                "grouped": true,
                "multiplexed": false,
                "read_format": read_format,
                "counter_scope": scope,
            },
            "events": event_metadata(),
            "workload": {
                "iterations": PROBE_ITERATIONS,
                "words": PROBE_WORDS,
                "checksum": checksum,
            },
            "sample": sample_json(delta),
            "diagnostics": diagnostics,
            "failure": Value::Null,
        }),
        Ok((delta, checksum, read_format, scope)) => json!({
            "schema": "mister-magik-pmu-probe-v1",
            "status": "failed",
            "target": {
                "architecture": std::env::consts::ARCH,
                "operating_system": std::env::consts::OS,
                "scope": "calling-thread-user-space",
                "grouped": true,
                "multiplexed": false,
                "read_format": read_format,
                "counter_scope": scope,
            },
            "events": event_metadata(),
            "workload": {
                "iterations": PROBE_ITERATIONS,
                "words": PROBE_WORDS,
                "checksum": checksum,
            },
            "sample": sample_json(delta),
            "diagnostics": diagnostics,
            "failure": {
                "stage": "validate-sample",
                "event": Value::Null,
                "errno": Value::Null,
                "message": "probe produced zero cycles or instructions",
            },
        }),
        Err(failure) => failed_probe(failure, diagnostics),
    }
}

fn measure_probe(
    group: Result<CounterGroup, PmuFailure>,
) -> Result<
    (
        CounterDelta,
        u64,
        mister_magik_perf_events::GroupReadFormat,
        mister_magik_perf_events::CounterScope,
    ),
    PmuFailure,
> {
    let group = group?;
    let read_format = group.read_format();
    let scope = group.scope();
    let span = group.span("pmu-probe")?;
    let mut words = vec![0x9e37_79b9_u32; PROBE_WORDS];
    let mut state = 0x243f_6a88_85a3_08d3_u64;
    for iteration in 0..PROBE_ITERATIONS {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let index = (state as usize) & (PROBE_WORDS - 1);
        let value = words[index].wrapping_add((state >> 32) as u32);
        words[index] = if state & 0x100 != 0 {
            value.rotate_left((state & 31) as u32)
        } else {
            value.rotate_right((state & 31) as u32)
        };
        std::hint::black_box(iteration);
    }
    let checksum = words
        .iter()
        .fold(state, |total, value| total.wrapping_add(u64::from(*value)));
    std::hint::black_box(checksum);
    Ok((span.finish()?.counters, checksum, read_format, scope))
}

fn valid_probe(delta: CounterDelta) -> bool {
    delta.counters.cycles > 0 && delta.counters.instructions > 0
}

fn sample_json(delta: CounterDelta) -> Value {
    json!({
        "time_enabled_ns": delta.time_enabled_ns,
        "time_running_ns": delta.time_running_ns,
        "counters": delta.counters,
        "derived": {
            "instructions_per_cycle": delta.instructions_per_cycle(),
            "cycles_per_instruction": delta.cycles_per_instruction(),
            "l1d_refill_pct": delta.l1d_refill_percent(),
            "branch_mispredict_pct": delta.branch_mispredict_percent(),
        },
    })
}

fn failed_probe(failure: PmuFailure, diagnostics: PmuOpenDiagnostics) -> Value {
    json!({
        "schema": "mister-magik-pmu-probe-v1",
        "status": "failed",
        "target": {
            "architecture": std::env::consts::ARCH,
            "operating_system": std::env::consts::OS,
            "scope": "calling-thread-user-space",
            "grouped": true,
            "multiplexed": false,
        },
        "events": event_metadata(),
        "workload": {
            "iterations": PROBE_ITERATIONS,
            "words": PROBE_WORDS,
        },
        "sample": Value::Null,
        "diagnostics": diagnostics,
        "failure": failure,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mister_magik_perf_events::CounterValues;

    #[test]
    fn sample_validation_requires_running_cycles_and_instructions() {
        let valid = CounterDelta {
            time_running_ns: 1,
            counters: CounterValues {
                cycles: 2,
                instructions: 1,
                ..CounterValues::default()
            },
            ..CounterDelta::default()
        };
        assert!(valid_probe(valid));
        assert!(!valid_probe(CounterDelta::default()));
    }

    #[test]
    fn failure_report_preserves_structured_details() {
        let report = failed_probe(
            PmuFailure {
                stage: "open-event".to_owned(),
                event: Some(mister_magik_perf_events::HardwareEvent::Cycles),
                errno: Some(13),
                message: "permission denied".to_owned(),
            },
            PmuOpenDiagnostics::default(),
        );
        assert_eq!(report["status"], "failed");
        assert_eq!(report["failure"]["stage"], "open-event");
        assert_eq!(report["failure"]["errno"], 13);
    }
}
