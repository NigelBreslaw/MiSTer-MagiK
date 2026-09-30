fn main() {
    println!("cargo:rerun-if-env-changed=MAGIK_MINI_BUILD_PROFILE");
    let profile =
        std::env::var("MAGIK_MINI_BUILD_PROFILE").unwrap_or_else(|_| "host-review".into());
    println!("cargo:rustc-env=MAGIK_MINI_BUILD_PROFILE={profile}");
    slint_build::compile_with_config(
        "ui/probe.slint",
        slint_build::CompilerConfiguration::new().with_debug_info(true),
    )
    .expect("compile Slint probe UI");
}
