use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub(crate) const PROGRESS_COPY_CHUNK_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PackSavePhase {
    Copy,
    Sync,
    Rename,
    ParentSync,
}

impl PackSavePhase {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Copy => "save",
            Self::Sync => "sync",
            Self::Rename => "rename",
            Self::ParentSync => "parent-sync",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PackSaveProgress {
    pub(crate) phase: PackSavePhase,
    pub(crate) bytes_done: u64,
    pub(crate) bytes_total: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PackSaveMetrics {
    pub(crate) bytes: u64,
    pub(crate) copy_ms: u64,
    pub(crate) sync_ms: u64,
    pub(crate) rename_ms: u64,
    pub(crate) parent_sync_ms: u64,
    pub(crate) total_ms: u64,
    pub(crate) progress_events: u64,
}

pub(crate) fn publish_pack_file_for_bench(
    source: &Path,
    final_path: &Path,
    mut progress: impl FnMut(PackSaveProgress),
) -> Result<PackSaveMetrics, String> {
    let result = publish_pack_file_impl(source, final_path, &mut progress);
    if result.is_err() {
        let _ = fs::remove_file(temp_path_for(final_path));
    }
    result
}

pub(crate) fn publish_pack_file_with_progress(
    source: &Path,
    final_path: &Path,
    progress: impl FnMut(PackSaveProgress),
) -> Result<PackSaveMetrics, String> {
    publish_pack_file_for_bench(source, final_path, progress)
}

fn publish_pack_file_impl(
    source: &Path,
    final_path: &Path,
    progress: &mut dyn FnMut(PackSaveProgress),
) -> Result<PackSaveMetrics, String> {
    let parent = final_path
        .parent()
        .ok_or_else(|| format!("pack destination has no parent: {}", final_path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("create pack destination parent {}: {e}", parent.display()))?;
    let tmp = temp_path_for(final_path);
    let _ = fs::remove_file(&tmp);
    let started = Instant::now();
    let mut metrics = PackSaveMetrics {
        bytes: file_len(source)?,
        ..Default::default()
    };
    let mut input = File::open(source).map_err(|e| format!("open {}: {e}", source.display()))?;
    let mut output = File::create(&tmp).map_err(|e| format!("create {}: {e}", tmp.display()))?;

    let copy_started = Instant::now();
    metrics.progress_events = copy_with_progress(&mut input, &mut output, metrics.bytes, progress)?;
    metrics.copy_ms = elapsed_ms(copy_started.elapsed());

    let bytes = metrics.bytes;
    emit_progress(&mut metrics, progress, PackSavePhase::Sync, bytes, bytes);
    let sync_started = Instant::now();
    output
        .sync_all()
        .map_err(|e| format!("sync {}: {e}", tmp.display()))?;
    metrics.sync_ms = elapsed_ms(sync_started.elapsed());
    drop(output);

    let bytes = metrics.bytes;
    emit_progress(&mut metrics, progress, PackSavePhase::Rename, bytes, bytes);
    let rename_started = Instant::now();
    fs::rename(&tmp, final_path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename {} -> {}: {e}", tmp.display(), final_path.display())
    })?;
    metrics.rename_ms = elapsed_ms(rename_started.elapsed());

    let bytes = metrics.bytes;
    emit_progress(
        &mut metrics,
        progress,
        PackSavePhase::ParentSync,
        bytes,
        bytes,
    );
    let parent_sync_started = Instant::now();
    sync_path(parent);
    metrics.parent_sync_ms = elapsed_ms(parent_sync_started.elapsed());
    metrics.total_ms = elapsed_ms(started.elapsed());
    Ok(metrics)
}

fn emit_progress(
    metrics: &mut PackSaveMetrics,
    progress: &mut dyn FnMut(PackSaveProgress),
    phase: PackSavePhase,
    bytes_done: u64,
    bytes_total: u64,
) {
    metrics.progress_events += 1;
    progress(PackSaveProgress {
        phase,
        bytes_done,
        bytes_total,
    });
}

pub(crate) fn temp_path_for(final_path: &Path) -> PathBuf {
    final_path.with_file_name(format!(
        "{}.tmp",
        final_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("screenshot-pack")
    ))
}

fn copy_with_progress(
    input: &mut File,
    output: &mut File,
    total: u64,
    progress: &mut dyn FnMut(PackSaveProgress),
) -> Result<u64, String> {
    let mut progress_events = 0u64;
    let mut bytes_done = 0u64;
    let mut buffer = vec![0u8; PROGRESS_COPY_CHUNK_BYTES];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|e| format!("progress copy read failed: {e}"))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|e| format!("progress copy write failed: {e}"))?;
        bytes_done += read as u64;
        progress_events += 1;
        progress(PackSaveProgress {
            phase: PackSavePhase::Copy,
            bytes_done,
            bytes_total: total,
        });
    }
    Ok(progress_events)
}

fn file_len(path: &Path) -> Result<u64, String> {
    path.metadata()
        .map(|meta| meta.len())
        .map_err(|e| format!("stat {}: {e}", path.display()))
}

fn sync_path(path: &Path) {
    match Command::new("sync").arg(path).status() {
        Ok(status) if status.success() => {}
        _ => {
            let _ = Command::new("sync").status();
        }
    };
}

fn elapsed_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn progress_save_writes_identical_bytes() {
        let dir = temp_dir("mister-magik-pack-save");
        let source = dir.join("source.bin");
        fs::write(&source, b"abcdef0123456789").unwrap();

        let final_path = dir.join("out.bin");
        let metrics = publish_pack_file_for_bench(&source, &final_path, |_| {}).unwrap();
        assert_eq!(fs::read(&final_path).unwrap(), b"abcdef0123456789");
        assert_eq!(metrics.bytes, 16);
        assert!(metrics.progress_events > 0);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn progress_mode_reports_monotonic_copy_bytes() {
        let dir = temp_dir("mister-magik-pack-save-progress");
        let source = dir.join("source.bin");
        fs::write(&source, vec![7u8; PROGRESS_COPY_CHUNK_BYTES + 17]).unwrap();
        let final_path = dir.join("out.bin");
        let mut copy_events = Vec::new();

        let metrics = publish_pack_file_for_bench(&source, &final_path, |event| {
            if event.phase == PackSavePhase::Copy {
                copy_events.push(event);
            }
        })
        .unwrap();

        assert_eq!(metrics.bytes, PROGRESS_COPY_CHUNK_BYTES as u64 + 17);
        assert!(copy_events.len() >= 2);
        assert_eq!(copy_events.last().unwrap().bytes_done, metrics.bytes);
        assert!(copy_events
            .windows(2)
            .all(|items| items[0].bytes_done < items[1].bytes_done));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_publish_preserves_existing_destination() {
        let dir = temp_dir("mister-magik-pack-save-failure");
        let missing_source = dir.join("missing.bin");
        let final_path = dir.join("existing.bin");
        fs::write(&final_path, b"old-pack").unwrap();

        let error = publish_pack_file_for_bench(&missing_source, &final_path, |_| {}).unwrap_err();

        assert!(error.contains("stat"));
        assert_eq!(fs::read(&final_path).unwrap(), b"old-pack");
        assert!(!temp_path_for(&final_path).exists());

        let _ = fs::remove_dir_all(dir);
    }
}
