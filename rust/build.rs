fn main() {
    let sources = [
        "ui/app.slint",
        "ui/bench/full_motion.slint",
        "ui/bench/static_ui.slint",
        "ui/bench/local_motion.slint",
        "ui/bench/text_heavy.slint",
        "ui/bench/solid_fill.slint",
        "ui/bench/list_scroll.slint",
    ];
    for path in sources {
        let config = slint_build::CompilerConfiguration::new()
            .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
        slint_build::compile_with_config(path, config)
            .unwrap_or_else(|e| panic!("Slint build failed for {path}: {e}"));
    }
}
