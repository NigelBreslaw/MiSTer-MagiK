fn main() {
    println!("cargo:rerun-if-changed=ui/main.slint");
    if std::env::var_os("CARGO_FEATURE_COMPILED_UI").is_some() {
        slint_build::compile("ui/main.slint").expect("compile Slint UI");
    }
}
