fn main() {
    // Embed glyphs/images into the binary for the software renderer, so we never
    // touch system fonts (no fontconfig) on the MiSTer.
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint build failed");
}
