//! Toolchain validation stage: prove a cross-compiled Rust binary runs on the
//! MiSTer's armv7 glibc userland. Next stages add /dev/mem FPGA access (porting
//! the proven SPI sequence from the Python spike, see AGENTS.md §9.5) and the
//! Slint software renderer.

fn main() {
    println!("hello from mister-slint-fb");
    println!("arch   = {}", std::env::consts::ARCH);
    println!("os     = {}", std::env::consts::OS);
    println!("pointer_width = {}", std::mem::size_of::<usize>() * 8);

    // Touch /dev/fb0 so we confirm filesystem + permissions on the device.
    match std::fs::metadata("/dev/fb0") {
        Ok(_) => println!("/dev/fb0 present"),
        Err(e) => println!("/dev/fb0 not accessible: {e}"),
    }
}
