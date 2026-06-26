use super::*;

pub(super) fn run_controller_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut MappedRgb565Framebuffer,
    window: &Rc<MinimalSoftwareWindow>,
    mut pad: PadPool,
    app: slint_ui::controller::ControllerTest,
    animation_clock: &AnimationClock,
) {
    let mut cached = vec![Rgb565Pixel(0); ui.render_w() * ui.render_h()];
    let start = Instant::now();
    let mut frames = 0u64;
    let mut pacer = VsyncPacer::from_env();
    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    println!(
        "controller_test running {label} — {} pad(s) connected",
        pad.len()
    );
    while secs == 0 || start.elapsed().as_secs() < secs {
        if pad.poll_with_debug_labels(true) {
            sync_bridge(&app, &pad);
            window.request_redraw();
        }
        update_slint_animations(animation_clock);
        let mut this_rect: Option<DirtyRect> = None;
        window.draw_if_needed(|renderer| {
            let region = renderer.render(&mut cached, ui.render_w());
            this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
        });
        let _pace = pacer.wait();
        if let Some(rect) = this_rect {
            copy_cached_rect_565(disp, ui, &cached, rect);
        }
        frames += 1;
    }
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
}
