#![allow(clippy::needless_return)]

#[path = "../preview_archive_bench.rs"]
mod preview_archive_bench;

fn main() {
    preview_archive_bench::run();
}
