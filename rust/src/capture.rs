use std::fs::File;
use std::io::{self, Read};

const DEFAULT_W: usize = 1920;
const DEFAULT_H: usize = 1080;
const DEFAULT_PNG: &str = "/tmp/fb0.png";
const DEFAULT_RAW: &str = "/tmp/fb0.raw";

pub fn run_png(args: &[String]) -> Result<(), String> {
    let (out, w, h) = parse_args(args, DEFAULT_PNG)?;
    let raw = read_fb(w, h).map_err(|e| format!("read /dev/fb0: {e}"))?;
    let png = bgrx_to_png(&raw, w, h);
    std::fs::write(&out, png).map_err(|e| format!("write {out}: {e}"))?;
    println!("captured /dev/fb0 -> {out} ({w}x{h} png)");
    Ok(())
}

pub fn run_raw(args: &[String]) -> Result<(), String> {
    let (out, w, h) = parse_args(args, DEFAULT_RAW)?;
    let raw = read_fb(w, h).map_err(|e| format!("read /dev/fb0: {e}"))?;
    std::fs::write(&out, raw).map_err(|e| format!("write {out}: {e}"))?;
    println!("captured /dev/fb0 -> {out} ({w}x{h} raw)");
    Ok(())
}

fn parse_args(args: &[String], default_out: &str) -> Result<(String, usize, usize), String> {
    let out = args
        .first()
        .cloned()
        .unwrap_or_else(|| default_out.to_string());
    let w = args
        .get(1)
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|e| format!("invalid width: {e}"))?
        .unwrap_or(DEFAULT_W);
    let h = args
        .get(2)
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|e| format!("invalid height: {e}"))?
        .unwrap_or(DEFAULT_H);
    if w == 0 || h == 0 {
        return Err("width and height must be non-zero".to_string());
    }
    Ok((out, w, h))
}

fn read_fb(w: usize, h: usize) -> io::Result<Vec<u8>> {
    let mut file = File::open("/dev/fb0")?;
    let mut raw = vec![0u8; w * h * 4];
    file.read_exact(&mut raw)?;
    Ok(raw)
}

fn bgrx_to_png(raw: &[u8], w: usize, h: usize) -> Vec<u8> {
    let rowstride = w * 4;
    let mut rgba = Vec::with_capacity((rowstride + 1) * h);
    for y in 0..h {
        rgba.push(0); // PNG filter: none.
        let row = &raw[y * rowstride..(y + 1) * rowstride];
        for px in row.chunks_exact(4) {
            rgba.push(px[2]);
            rgba.push(px[1]);
            rgba.push(px[0]);
            rgba.push(0xff);
        }
    }

    let mut png = Vec::new();
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // RGBA, 8-bit, deflate, no interlace.
    write_chunk(&mut png, b"IHDR", &ihdr);
    write_chunk(&mut png, b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&rgba, 1));
    write_chunk(&mut png, b"IEND", &[]);
    png
}

fn write_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let mut crc_data = Vec::with_capacity(tag.len() + data.len());
    crc_data.extend_from_slice(tag);
    crc_data.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_data).to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}
