#![allow(clippy::all, unused_imports)]

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
