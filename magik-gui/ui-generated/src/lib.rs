#![allow(clippy::all, unused_imports)]

#[cfg(all(not(mister_ui_scope_launcher), mister_bench_scenes))]
pub mod app {
    include!(concat!(env!("OUT_DIR"), "/app.rs"));
}

#[cfg(mister_bench_scenes)]
pub mod full_motion {
    include!(concat!(env!("OUT_DIR"), "/full_motion.rs"));
}

#[cfg(mister_bench_scenes)]
pub mod static_ui {
    include!(concat!(env!("OUT_DIR"), "/static_ui.rs"));
}

#[cfg(mister_bench_scenes)]
pub mod local_motion {
    include!(concat!(env!("OUT_DIR"), "/local_motion.rs"));
}

#[cfg(mister_bench_scenes)]
pub mod effect_hud {
    include!(concat!(env!("OUT_DIR"), "/effect_hud.rs"));
}

#[cfg(all(feature = "video", mister_bench_scenes))]
pub mod video_playback {
    include!(concat!(env!("OUT_DIR"), "/video_playback.rs"));
}

pub mod controller {
    include!(concat!(env!("OUT_DIR"), "/controller_test.rs"));
}

pub mod launcher {
    include!(concat!(env!("OUT_DIR"), "/launcher.rs"));
}
