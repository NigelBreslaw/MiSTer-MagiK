//! Capture Main's active display before suspension; never infer it afterwards.
use mister_magik_core::display::{DisplayGeometry, ResolvedDisplayPlan};
pub fn snapshot() -> Result<String, String> {
    let reply = crate::main_control::request("mister_magik_display_get_v1\n")?;
    let field = |key: &str| reply.split_whitespace().find_map(|v| v.strip_prefix(key));
    if field("pending=") != Some("none") {
        return Err("display transaction pending".into());
    }
    let mode = field("active=").ok_or("missing active display mode")?;
    let detected = if matches!(mode, "auto" | "custom") {
        if !crate::device::status()?
            .get("launcher_active")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return Err(
                "auto/custom display geometry requires active Main before suspension".into(),
            );
        }
        detected_geometry()?
    } else {
        None
    };
    let plan = ResolvedDisplayPlan::from_mode_or_detected(mode, detected)
        .ok_or("cannot resolve Main display plan")?;
    Ok(format!("{mode},{},{}", plan.output_w, plan.output_h))
}
#[cfg(target_os = "linux")]
fn detected_geometry() -> Result<Option<DisplayGeometry>, String> {
    let mut fpga = mister_magik_mister_runtime::fpga::Fpga::open().map_err(|e| e.to_string())?;
    let v = fpga.read_video_info().map_err(|e| e.to_string())?;
    Ok(DisplayGeometry::from_video_words(
        v.width, v.height, v.de_h, v.de_v,
    ))
}
#[cfg(not(target_os = "linux"))]
fn detected_geometry() -> Result<Option<DisplayGeometry>, String> {
    Ok(None)
}
