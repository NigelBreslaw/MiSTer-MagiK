use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

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

pub fn run_tone_from_args(args: &[String]) -> Result<(), String> {
    let secs = args
        .first()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(3.0)
        .max(0.1);
    let hz = args
        .get(1)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(440.0)
        .clamp(20.0, 20_000.0);
    let amp = args
        .get(2)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.20)
        .clamp(0.0, 1.0);

    println!("audio-tone: {secs:.2}s {hz:.1}Hz amp={amp:.2} via {DEFAULT_DEVICE}");
    if let Ok(status) = read_status() {
        print!("audio-tone before: {status}");
    }

    let mut sink = MrAudioSink::open_default()?;
    let total_frames = (secs * SAMPLE_RATE as f64).round() as usize;
    let chunk_frames = 1024usize;
    let mut phase = 0.0f64;
    let phase_step = std::f64::consts::TAU * hz / SAMPLE_RATE as f64;
    let mut remaining = total_frames;
    let started = Instant::now();

    while remaining > 0 {
        let n = remaining.min(chunk_frames);
        let mut chunk = Vec::with_capacity(n * CHANNELS);
        for _ in 0..n {
            let sample = (phase.sin() * amp * i16::MAX as f64) as i16;
            chunk.push(sample);
            chunk.push(sample);
            phase += phase_step;
            if phase >= std::f64::consts::TAU {
                phase -= std::f64::consts::TAU;
            }
        }
        sink.write_frames(&chunk)?;
        remaining -= n;

        let target_elapsed =
            Duration::from_secs_f64(sink.frames_written() as f64 / SAMPLE_RATE as f64);
        if let Some(wait) = target_elapsed.checked_sub(started.elapsed()) {
            thread::sleep(wait);
        }
    }

    println!(
        "audio-tone wrote {} frames in {:.2}s",
        sink.frames_written(),
        started.elapsed().as_secs_f64()
    );
    if let Ok(status) = read_status() {
        print!("audio-tone after: {status}");
    }
    Ok(())
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
