fn main() {
    slint_build::compile_with_config(
        "ui/probe.slint",
        slint_build::CompilerConfiguration::new().with_debug_info(true),
    )
    .expect("compile Slint probe UI");
}
