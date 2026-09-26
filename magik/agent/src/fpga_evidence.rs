//! Incident capture never installs, restarts, arms or clears anything.
use serde_json::Value;
#[cfg(not(target_os = "linux"))]
pub fn capture() -> Result<Value, String> {
    Err("FPGA evidence requires Linux hardware".into())
}
#[cfg(target_os = "linux")]
pub fn capture() -> Result<Value, String> {
    use mister_magik_mister_runtime::{fpga::Fpga, video_evidence};
    use serde_json::json;
    let identity = || {
        json!({"boot_id":std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok(),
        "service_pid":std::process::id(),"main_status":super::device::status().ok()})
    };
    let started = std::time::Instant::now();
    let before = identity();
    let mut fpga = Fpga::open().map_err(|e| e.to_string())?;
    let mut samples = Vec::new();
    let latch_before = video_evidence::capture_latch(&mut fpga)
        .unwrap_or_else(|error| json!({"error":error.to_string()}));
    for _ in 0..3 {
        for first in [true, false] {
            let sample_started_us = started.elapsed().as_micros();
            let mut sample = match video_evidence::capture(&mut fpga, first) {
                Ok(value) => value,
                Err(error) => json!({"first_requested":first,"error":error.to_string()}),
            };
            sample["capture_start_us"] = json!(sample_started_us);
            sample["capture_end_us"] = json!(started.elapsed().as_micros());
            samples.push(sample);
        }
    }
    let latch_after = video_evidence::capture_latch(&mut fpga)
        .unwrap_or_else(|error| json!({"error":error.to_string()}));
    Ok(
        json!({"latch_before":latch_before,"latch_after":latch_after,"read_only":true,"before":before,"samples":samples,"after":identity()}),
    )
}
