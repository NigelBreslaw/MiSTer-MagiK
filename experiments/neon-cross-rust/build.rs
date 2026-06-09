use std::env;
use std::path::PathBuf;
use std::process::Command;

fn env_tool(target: &str, name: &str, fallback: &str) -> String {
    let normalized = target.replace('-', "_");
    env::var(format!("{name}_{normalized}"))
        .or_else(|_| env::var(format!("TARGET_{normalized}_{name}")))
        .or_else(|_| env::var(name))
        .unwrap_or_else(|_| fallback.to_string())
}

fn run(mut command: Command) {
    let printable = format!("{command:?}");
    let status = command
        .status()
        .unwrap_or_else(|err| panic!("failed to run {printable}: {err}"));
    assert!(status.success(), "{printable} exited with {status}");
}

fn main() {
    let target = env::var("TARGET").expect("TARGET is set by Cargo");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let obj = out_dir.join("neon_probe.o");
    let lib = out_dir.join("libneon_probe.a");

    if target != "armv7-unknown-linux-gnueabihf" {
        panic!("this experiment is only intended for armv7-unknown-linux-gnueabihf");
    }

    let cc = env_tool(&target, "CC", "arm-linux-gnueabihf-gcc");
    let ar = env_tool(&target, "AR", "arm-linux-gnueabihf-ar");
    println!("cargo:warning=Compiling C NEON probe with {cc}");

    let mut compile = Command::new(&cc);
    compile.args([
        "-O3",
        "-mcpu=cortex-a9",
        "-mfpu=neon",
        "-mfloat-abi=hard",
        "-fPIC",
        "-ffunction-sections",
        "-fdata-sections",
        "-c",
        "src/neon_probe.c",
        "-o",
    ]);
    compile.arg(&obj);
    run(compile);

    let mut archive = Command::new(&ar);
    archive.arg("crs").arg(&lib).arg(&obj);
    run(archive);

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=neon_probe");
    println!("cargo:rerun-if-changed=src/neon_probe.c");
}
