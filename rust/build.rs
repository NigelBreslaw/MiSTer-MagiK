fn main() {
    // Press Start 2P at 960×540 design sizes (×2 when MISTER_RENDER_SCALE=2).
    if std::env::var("SLINT_FONT_SIZES").is_err() {
        std::env::set_var("SLINT_FONT_SIZES", "8,16,24");
    }

    let sources = [
        "ui/app.slint",
        "ui/bench/full_motion.slint",
        "ui/bench/static_ui.slint",
        "ui/bench/local_motion.slint",
        "ui/bench/text_heavy.slint",
        "ui/bench/solid_fill.slint",
        "ui/bench/list_scroll.slint",
        "ui/controller_test.slint",
        "ui/launcher.slint",
    ];
    for path in sources {
        let config = slint_build::CompilerConfiguration::new()
            .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
        slint_build::compile_with_config(path, config)
            .unwrap_or_else(|e| panic!("Slint build failed for {path}: {e}"));
    }
}
