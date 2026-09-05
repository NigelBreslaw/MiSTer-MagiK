//! Optional CPU sampling for one uniquely identified measurement window.
use std::path::PathBuf;
pub struct CpuProfile {
    guard: pprof::ProfilerGuard<'static>,
    root: PathBuf,
}
impl CpuProfile {
    pub fn start() -> Result<Option<Self>, String> {
        let Some(root) = std::env::var_os("MISTER_MAGIK2_PROFILE_DIR") else {
            return Ok(None);
        };
        let root = PathBuf::from(root);
        if let Some(parent) = root.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        // Refuse a reused run directory: a previous profile must never pass this run.
        std::fs::create_dir(&root).map_err(|e| e.to_string())?;
        let guard = pprof::ProfilerGuardBuilder::default()
            .frequency(99)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Some(Self { guard, root }))
    }
    pub fn finish(self) -> Result<(), String> {
        let report = self.guard.report().build().map_err(|e| e.to_string())?;
        let mut lines = Vec::new();
        let mut samples = 0;
        for (frames, count) in &report.data {
            if *count <= 0 {
                continue;
            }
            let mut stack = vec![frames.thread_name_or_id()];
            for frame in frames.frames.iter().rev() {
                for symbol in frame.iter().rev() {
                    stack.push(symbol.name().replace(';', ":"));
                }
            }
            if stack.len() > 1 {
                lines.push(format!("{} {count}", stack.join(";")));
                samples += count;
            }
        }
        if samples == 0 {
            return Err("CPU sampler collected no stacks".into());
        }
        lines.sort();
        std::fs::write(self.root.join("profile.folded"), lines.join("\n"))
            .map_err(|e| e.to_string())?;
        report
            .flamegraph(
                std::fs::File::create(self.root.join("flamegraph.svg"))
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        let status = serde_json::json!({"complete":true,"samples":samples,"run_id":self.root.file_name().and_then(|s|s.to_str()),"sha256":std::env::var("MISTER_MAGIK2_ARTIFACT_SHA256").unwrap_or_default()});
        std::fs::write(self.root.join("profile.json"), status.to_string())
            .map_err(|e| e.to_string())
    }
}
