use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

pub const DEFAULT_DEVICE: &str = "/dev/MrAudio";
pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;

pub struct MrAudioSink {
    file: File,
    frames_written: u64,
}

impl MrAudioSink {
    pub fn open_default() -> Result<Self, String> {
        Self::open(DEFAULT_DEVICE)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let file = OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|e| format!("open {}: {e}", path.display()))?;
        Ok(Self {
            file,
            frames_written: 0,
        })
    }

    pub fn write_frames(&mut self, frames: &[i16]) -> Result<usize, String> {
        if frames.len() % CHANNELS != 0 {
            return Err(format!(
                "audio frame buffer has {} samples, expected stereo pairs",
                frames.len()
            ));
        }
        let bytes = samples_as_le_bytes(frames);
        self.file
            .write_all(bytes)
            .map_err(|e| format!("write {DEFAULT_DEVICE}: {e}"))?;
        let written = frames.len() / CHANNELS;
        self.frames_written += written as u64;
        Ok(written)
    }

    pub fn frames_written(&self) -> u64 {
        self.frames_written
    }
}

pub fn read_status() -> Result<String, String> {
    let mut s = String::new();
    OpenOptions::new()
        .read(true)
        .open(DEFAULT_DEVICE)
        .map_err(|e| format!("open {DEFAULT_DEVICE}: {e}"))?
        .read_to_string(&mut s)
        .map_err(|e| format!("read {DEFAULT_DEVICE}: {e}"))?;
    Ok(s)
}

fn samples_as_le_bytes(samples: &[i16]) -> &[u8] {
    debug_assert!(cfg!(target_endian = "little"));
    // SAFETY: u8 has alignment 1, and the returned byte slice is tied to the
    // input slice lifetime. The caller only uses this on little-endian targets.
    unsafe {
        std::slice::from_raw_parts(
            samples.as_ptr() as *const u8,
            std::mem::size_of_val(samples),
        )
    }
}
