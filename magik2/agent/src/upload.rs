//! One verified temporary file; upload memory is independent of artifact size.
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub struct Staged {
    path: PathBuf,
}
impl Drop for Staged {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
impl Staged {
    pub fn publish(&self, destination: &Path) -> Result<(), String> {
        fs::rename(&self.path, destination).map_err(|e| e.to_string())
    }
}

pub fn receive(
    reader: &mut impl Read,
    root: &Path,
    artifact: &str,
    hash: &str,
    length: usize,
    id: &str,
) -> Result<Staged, String> {
    if !matches!(artifact, "probe" | "mister-magik2-agent")
        || hash.len() != 64
        || !hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("invalid artifact or SHA-256".into());
    }
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let request_hash = hex(&Sha256::digest(id.as_bytes()));
    let staged = Staged {
        path: root.join(format!(".{artifact}.{}.part", &request_hash[..16])),
    };
    let mut output = File::create(&staged.path).map_err(|e| e.to_string())?;
    let mut remaining = length;
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    while remaining > 0 {
        let requested = remaining.min(buffer.len());
        let n = reader
            .read(&mut buffer[..requested])
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("truncated upload".into());
        }
        output.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        digest.update(&buffer[..n]);
        remaining -= n;
    }
    if hex(&digest.finalize()) != hash {
        return Err("sha256 mismatch".into());
    }
    output.sync_all().map_err(|e| e.to_string())?;
    output
        .set_permissions(fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_upload_is_removed_without_publication() {
        let root = std::env::temp_dir().join(format!("magik2-partial-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let bytes = b"complete bytes";
        let hash = hex(&Sha256::digest(bytes));
        assert!(
            receive(
                &mut &bytes[..3],
                &root,
                "probe",
                &hash,
                bytes.len(),
                "partial"
            )
            .is_err()
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        let staged = receive(
            &mut &bytes[..],
            &root,
            "probe",
            &hash,
            bytes.len(),
            "complete",
        )
        .unwrap();
        assert!(!root.join("probe").exists());
        staged.publish(&root.join("probe")).unwrap();
        assert_eq!(fs::read(root.join("probe")).unwrap(), bytes);
        drop(staged);
        fs::remove_dir_all(root).unwrap();
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
