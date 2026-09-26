//! Fixed read-only FPGA evidence operations. Never accepts caller-supplied opcodes.
use crate::fpga::Fpga;
use mister_magik_latch_contract::video_evidence as contract;
use serde_json::{Value, json};
use std::io;
use std::time::{Duration, Instant};

fn wire_read(
    fpga: &mut Fpga,
    command: u16,
    magic: u16,
    count: usize,
) -> io::Result<Option<Vec<(u16, u16)>>> {
    let result = (|| {
        let ack = fpga.cmd_capture(command)?;
        if ack.0 != magic && ack.1 != magic {
            return Ok(None);
        }
        (0..count)
            .map(|_| fpga.spi_capture(0))
            .collect::<io::Result<Vec<_>>>()
            .map(Some)
    })();
    fpga.disable_io();
    result
}

fn snapshot(fpga: &mut Fpga) -> io::Result<Vec<(u16, u16)>> {
    for _ in 0..64 {
        if let Some(words) = wire_read(fpga, contract::READ_SNAPSHOT, 0x4d5a, contract::WORDS)? {
            return Ok(words);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "FPGA evidence snapshot unavailable",
    ))
}

pub fn capture(fpga: &mut Fpga, first: bool) -> io::Result<Value> {
    let started = Instant::now();
    let _guard = fpga.lock_evidence_transaction()?;
    // Probe the new read-only selector first; old cores ignore it. This avoids
    // interpreting the shared 0x68 acknowledgement as proof of a new schema.
    let raw = if wire_read(fpga, contract::SELECT_LIVE, 0x4d59, 0)?.is_some() {
        let live = snapshot(fpga)?;
        if first {
            if wire_read(fpga, contract::SELECT_FIRST, 0x4d58, 0)?.is_none() {
                return Err(io::Error::other("FPGA first-event selector unavailable"));
            }
            snapshot(fpga)?
        } else {
            live
        }
    } else {
        wire_read(fpga, contract::SELECT_FIRST, 0x4d58, 4)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::Unsupported, "FPGA evidence unavailable")
        })?
    };
    let high: Vec<_> = raw.iter().map(|v| v.0).collect();
    let low: Vec<_> = raw.iter().map(|v| v.1).collect();
    let decoded_high = contract::decode(&high);
    let decoded_low = contract::decode(&low);
    let decoded = match (decoded_high, decoded_low) {
        (Ok(a), Ok(b)) if a != b => Err("ACK phases contain different valid evidence"),
        (Ok(a), _) | (_, Ok(a)) => Ok(a),
        (Err(a), Err(_)) => Err(a),
    };
    let mut result = json!({"read_only":true,"elapsed_us":started.elapsed().as_micros(),"raw_ack_high":high,"raw_ack_low":low});
    match decoded {
        Ok(e) => {
            result["decoded"] = json!({"schema":e.schema,"record_valid":e.record_valid,
            "first_selected":e.first_selected,"ledger_valid":e.ledger_valid,"cause":e.cause,
            "physical_depth":e.physical_depth,"physical_phase":e.physical_phase,
            "production_depth":e.production_depth,"production_phase":e.production_phase,
            "output_state":e.output_state,"flags":e.flags,"crc_valid":true,
            "output_is_later_snapshot":e.schema==24,
            "attribution":if e.schema==23 {"observer_invalid"} else {"boundary_evidence_only"}})
        }
        Err(error) => result["decode_error"] = json!(error),
    }
    Ok(result)
}

/// Read latch context under the same bounded incident lock. These are only
/// capability/status reads; no framebuffer, reset, rearm or latch-post command.
pub fn capture_latch(fpga: &mut Fpga) -> io::Result<Value> {
    let _guard = fpga.lock_evidence_transaction()?;
    let (high, low, caps) = fpga.read_magik_latched_fbuf_capabilities()?;
    let status = match fpga.read_magik_latched_fbuf_status_sample() {
        Ok(sample) => {
            let s = sample.status;
            json!({"wire": sample.diagnostics, "active_sequence":s.active_sequence,
                "pending_sequence":s.pending_sequence,"flags":s.flags,"flip_count":s.flip_count,
                "post_count":s.post_count,"drop_count":s.drop_count,"reject_count":s.reject_count,
                "active_base":s.active_base,"active_width":s.active_width,"active_height":s.active_height,
                "active_stride":s.active_stride,"active_route_epoch":s.active_route_epoch,
                "active_transaction":s.active_transaction,"pending_transaction":s.pending_transaction,
                "accepted_transaction":s.accepted_transaction,"accepted_sequence":s.accepted_sequence})
        }
        Err(error) => json!({"error":error.to_string(),"wire":error.diagnostics}),
    };
    Ok(json!({"capability_ack_high":high,"capability_ack_low":low,
        "capabilities":format!("{caps:?}"),"status":status}))
}
