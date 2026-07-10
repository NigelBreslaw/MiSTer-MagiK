use slint::platform::software_renderer::Rgb565Pixel;
use std::sync::OnceLock;
#[cfg(feature = "bench-tools")]
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub struct Rgb565FrameView<'a> {
    pub pixels: &'a [Rgb565Pixel],
    pub width: usize,
    pub height: usize,
    pub stride_pixels: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownsampledGeometry {
    pub width: usize,
    pub height: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DownsampleImplementation {
    Scalar,
    Neon,
}

pub const fn compiled_implementation() -> DownsampleImplementation {
    #[cfg(all(target_arch = "arm", any(target_feature = "neon", mister_arm_neon)))]
    {
        DownsampleImplementation::Neon
    }
    #[cfg(not(all(target_arch = "arm", any(target_feature = "neon", mister_arm_neon))))]
    {
        DownsampleImplementation::Scalar
    }
}

impl DownsampleImplementation {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Scalar => "scalar",
            Self::Neon => "neon",
        }
    }
}

pub fn configured_implementation() -> DownsampleImplementation {
    static IMPLEMENTATION: OnceLock<DownsampleImplementation> = OnceLock::new();
    *IMPLEMENTATION.get_or_init(|| {
        match std::env::var("MISTER_FRAMEBUFFER_STREAM_SIMD").as_deref() {
            Ok("scalar") | Ok("SCALAR") => DownsampleImplementation::Scalar,
            Ok("neon") | Ok("NEON")
                if compiled_implementation() == DownsampleImplementation::Neon =>
            {
                DownsampleImplementation::Neon
            }
            _ => auto_implementation(),
        }
    })
}

const fn auto_implementation() -> DownsampleImplementation {
    #[cfg(all(target_arch = "arm", not(target_feature = "neon"), mister_arm_neon))]
    {
        // The device gate measured the fixed-target scalar kernel faster than
        // explicit NEON on Cortex-A9. Keep NEON compiled and benchmarkable,
        // but do not make an unproven performance path the automatic choice.
        DownsampleImplementation::Scalar
    }
    #[cfg(not(all(target_arch = "arm", not(target_feature = "neon"), mister_arm_neon)))]
    {
        compiled_implementation()
    }
}

pub fn downsample_rgb565_2x(
    source: Rgb565FrameView<'_>,
    destination: &mut Vec<Rgb565Pixel>,
) -> Result<DownsampledGeometry, &'static str> {
    downsample_rgb565_2x_with(source, destination, configured_implementation())
}

fn downsample_rgb565_2x_with(
    source: Rgb565FrameView<'_>,
    destination: &mut Vec<Rgb565Pixel>,
    implementation: DownsampleImplementation,
) -> Result<DownsampledGeometry, &'static str> {
    validate_source(source)?;
    let geometry = DownsampledGeometry {
        width: source.width.div_ceil(2),
        height: source.height.div_ceil(2),
    };
    let output_len = geometry
        .width
        .checked_mul(geometry.height)
        .ok_or("downsampled RGB565 geometry overflows")?;
    destination.resize(output_len, Rgb565Pixel(0));

    #[cfg(all(target_arch = "arm", any(target_feature = "neon", mister_arm_neon)))]
    if implementation == DownsampleImplementation::Neon {
        // SAFETY: validate_source proves every selected source row contains
        // source.width pixels, destination was resized for every output row,
        // and the helper checks its 16-pixel vector tail before each load.
        unsafe { downsample_rgb565_2x_neon(source, destination, geometry) };
        return Ok(geometry);
    }

    #[cfg(all(target_arch = "arm", not(target_feature = "neon"), mister_arm_neon))]
    if implementation == DownsampleImplementation::Scalar {
        // SAFETY: the same validated geometry guarantees used by the NEON
        // branch apply to the fixed-target scalar reference implementation.
        unsafe { downsample_rgb565_2x_fixed_scalar(source, destination, geometry) };
        return Ok(geometry);
    }

    let _ = implementation;
    downsample_rgb565_2x_scalar(source, destination, geometry);
    Ok(geometry)
}

#[cfg(feature = "bench-tools")]
#[derive(Clone, Debug)]
struct BenchResult {
    implementation: DownsampleImplementation,
    checksum: u64,
    p50: Duration,
    p95: Duration,
    max: Duration,
}

#[cfg(feature = "bench-tools")]
#[derive(Clone, Debug)]
struct BenchCaseResult {
    name: &'static str,
    scalar: BenchResult,
    neon: BenchResult,
}

#[cfg(feature = "bench-tools")]
const SIMD_BENCH_DEFAULT_SAMPLES: usize = 200;

#[cfg(feature = "bench-tools")]
const SIMD_BENCH_WARMUP_SAMPLES: usize = 20;

/// Compare scalar and compiled-NEON RGB565 decimation on deterministic inputs.
///
/// This is intentionally a bench-tools-only device command rather than a
/// production hot-path branch. It proves both output identity and the exact
/// implementation compiled into the ARM binary.
#[cfg(feature = "bench-tools")]
pub fn run_simd_bench() -> bool {
    let samples = std::env::var("MISTER_STREAM_SIMD_BENCH_SAMPLES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(SIMD_BENCH_DEFAULT_SAMPLES);
    let cases = [
        ("full_960x540", 960usize, 540usize, 960usize),
        ("padded_960x540", 960usize, 540usize, 976usize),
        ("odd_959x539", 959usize, 539usize, 967usize),
    ];
    let mut results = Vec::with_capacity(cases.len());

    for (name, width, height, stride_pixels) in cases {
        let pixels = deterministic_pixels(stride_pixels * height);
        let source = Rgb565FrameView {
            pixels: &pixels,
            width,
            height,
            stride_pixels,
        };
        let scalar = benchmark_implementation(source, DownsampleImplementation::Scalar, samples);
        let neon = benchmark_implementation(source, DownsampleImplementation::Neon, samples);
        log_bench_result(name, width, height, stride_pixels, samples, &scalar);
        log_bench_result(name, width, height, stride_pixels, samples, &neon);
        results.push(BenchCaseResult { name, scalar, neon });
    }

    let checksums_identical = results
        .iter()
        .all(|result| result.scalar.checksum == result.neon.checksum);
    let full = results
        .iter()
        .find(|result| result.name == "full_960x540")
        .expect("full SIMD benchmark case");
    let speedup = duration_ratio(full.scalar.p95, full.neon.p95);
    let compiled = compiled_implementation();
    let passed = compiled == DownsampleImplementation::Neon
        && configured_implementation() == DownsampleImplementation::Neon
        && checksums_identical
        && full.neon.p95 <= Duration::from_millis(4)
        && full.neon.max <= Duration::from_millis(6)
        && speedup >= 1.5;

    crate::ui_logln!(
        "framebuffer_stream_simd_gate_tsv\tcompiled_implementation={}\tauto_implementation={}\tchecksums_identical={}\thalf_snapshot_neon_p95_us={}\thalf_snapshot_neon_max_us={}\thalf_snapshot_scalar_p95_us={}\tspeedup={speedup:.3}\tpassed={}",
        compiled.label(),
        configured_implementation().label(),
        u8::from(checksums_identical),
        full.neon.p95.as_micros(),
        full.neon.max.as_micros(),
        full.scalar.p95.as_micros(),
        u8::from(passed),
    );
    passed
}

#[cfg(feature = "bench-tools")]
fn deterministic_pixels(len: usize) -> Vec<Rgb565Pixel> {
    let mut state = 0x1234_5678u32;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            Rgb565Pixel((state >> 16) as u16)
        })
        .collect()
}

#[cfg(feature = "bench-tools")]
fn benchmark_implementation(
    source: Rgb565FrameView<'_>,
    implementation: DownsampleImplementation,
    samples: usize,
) -> BenchResult {
    let mut destination = Vec::new();
    for _ in 0..SIMD_BENCH_WARMUP_SAMPLES {
        downsample_rgb565_2x_with(source, &mut destination, implementation)
            .expect("valid SIMD benchmark geometry");
        std::hint::black_box(&destination);
    }

    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        downsample_rgb565_2x_with(source, &mut destination, implementation)
            .expect("valid SIMD benchmark geometry");
        durations.push(started.elapsed());
        std::hint::black_box(&destination);
    }
    durations.sort_unstable();

    BenchResult {
        implementation,
        checksum: checksum_rgb565(&destination),
        p50: percentile_duration(&durations, 50),
        p95: percentile_duration(&durations, 95),
        max: durations.last().copied().unwrap_or_default(),
    }
}

#[cfg(feature = "bench-tools")]
fn log_bench_result(
    name: &str,
    width: usize,
    height: usize,
    stride_pixels: usize,
    samples: usize,
    result: &BenchResult,
) {
    crate::ui_logln!(
        "framebuffer_stream_simd_bench_tsv\tcase={name}\twidth={width}\theight={height}\tstride_pixels={stride_pixels}\timplementation={}\tsamples={samples}\tchecksum={:016x}\tp50_us={}\tp95_us={}\tmax_us={}",
        result.implementation.label(),
        result.checksum,
        result.p50.as_micros(),
        result.p95.as_micros(),
        result.max.as_micros(),
    );
}

#[cfg(feature = "bench-tools")]
fn checksum_rgb565(pixels: &[Rgb565Pixel]) -> u64 {
    pixels.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, pixel| {
        let hash = hash ^ u64::from(pixel.0 & 0xff);
        let hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        let hash = hash ^ u64::from(pixel.0 >> 8);
        hash.wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(feature = "bench-tools")]
fn percentile_duration(sorted: &[Duration], percentile: usize) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = percentile
        .saturating_mul(sorted.len())
        .div_ceil(100)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[rank]
}

#[cfg(feature = "bench-tools")]
fn duration_ratio(numerator: Duration, denominator: Duration) -> f64 {
    if denominator.is_zero() {
        return f64::INFINITY;
    }
    numerator.as_secs_f64() / denominator.as_secs_f64()
}

fn validate_source(source: Rgb565FrameView<'_>) -> Result<(), &'static str> {
    if source.width == 0 || source.height == 0 || source.stride_pixels < source.width {
        return Err("invalid RGB565 source geometry");
    }
    let required = source
        .stride_pixels
        .checked_mul(source.height)
        .ok_or("RGB565 source geometry overflows")?;
    if source.pixels.len() < required {
        return Err("RGB565 source buffer is shorter than its geometry");
    }
    Ok(())
}

fn downsample_rgb565_2x_scalar(
    source: Rgb565FrameView<'_>,
    destination: &mut [Rgb565Pixel],
    geometry: DownsampledGeometry,
) {
    for output_y in 0..geometry.height {
        let source_y = output_y * 2;
        let source_start = source_y * source.stride_pixels;
        let source_row = &source.pixels[source_start..source_start + source.width];
        let destination_start = output_y * geometry.width;
        let destination_row =
            &mut destination[destination_start..destination_start + geometry.width];
        for (output_x, pixel) in destination_row.iter_mut().enumerate() {
            *pixel = source_row[output_x * 2];
        }
    }
}

#[cfg(all(target_arch = "arm", target_feature = "neon"))]
unsafe fn downsample_rgb565_2x_neon(
    source: Rgb565FrameView<'_>,
    destination: &mut [Rgb565Pixel],
    geometry: DownsampledGeometry,
) {
    use core::arch::arm::{vld2q_u16, vst1q_u16};

    for output_y in 0..geometry.height {
        let source_y = output_y * 2;
        let source_row = source
            .pixels
            .as_ptr()
            .add(source_y * source.stride_pixels)
            .cast::<u16>();
        let destination_row = destination
            .as_mut_ptr()
            .add(output_y * geometry.width)
            .cast::<u16>();
        let mut output_x = 0usize;
        while output_x + 8 <= geometry.width && output_x * 2 + 16 <= source.width {
            let separated = vld2q_u16(source_row.add(output_x * 2));
            vst1q_u16(destination_row.add(output_x), separated.0);
            output_x += 8;
        }
        while output_x < geometry.width {
            *destination_row.add(output_x) = *source_row.add(output_x * 2);
            output_x += 1;
        }
    }
}

#[cfg(all(target_arch = "arm", not(target_feature = "neon"), mister_arm_neon))]
unsafe fn downsample_rgb565_2x_fixed_scalar(
    source: Rgb565FrameView<'_>,
    destination: &mut [Rgb565Pixel],
    geometry: DownsampledGeometry,
) {
    unsafe extern "C" {
        fn mister_magik_downsample_rgb565_2x_scalar(
            source: *const u16,
            source_height: usize,
            source_stride: usize,
            destination: *mut u16,
            destination_width: usize,
        );
    }

    mister_magik_downsample_rgb565_2x_scalar(
        source.pixels.as_ptr().cast::<u16>(),
        source.height,
        source.stride_pixels,
        destination.as_mut_ptr().cast::<u16>(),
        geometry.width,
    );
}

#[cfg(all(target_arch = "arm", not(target_feature = "neon"), mister_arm_neon))]
unsafe fn downsample_rgb565_2x_neon(
    source: Rgb565FrameView<'_>,
    destination: &mut [Rgb565Pixel],
    geometry: DownsampledGeometry,
) {
    unsafe extern "C" {
        fn mister_magik_downsample_rgb565_2x_neon(
            source: *const u16,
            source_width: usize,
            source_height: usize,
            source_stride: usize,
            destination: *mut u16,
            destination_width: usize,
        );
    }

    mister_magik_downsample_rgb565_2x_neon(
        source.pixels.as_ptr().cast::<u16>(),
        source.width,
        source.height,
        source.stride_pixels,
        destination.as_mut_ptr().cast::<u16>(),
        geometry.width,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(
        pixels: &[Rgb565Pixel],
        width: usize,
        height: usize,
        stride_pixels: usize,
    ) -> Rgb565FrameView<'_> {
        Rgb565FrameView {
            pixels,
            width,
            height,
            stride_pixels,
        }
    }

    #[test]
    fn scalar_downsample_keeps_top_left_pixel_of_each_two_by_two_block() {
        let pixels = (0..24).map(Rgb565Pixel).collect::<Vec<_>>();
        let mut output = Vec::new();

        let geometry = downsample_rgb565_2x_with(
            view(&pixels, 6, 4, 6),
            &mut output,
            DownsampleImplementation::Scalar,
        )
        .expect("downsample even frame");

        assert_eq!(
            geometry,
            DownsampledGeometry {
                width: 3,
                height: 2
            }
        );
        assert_eq!(
            output,
            vec![
                Rgb565Pixel(0),
                Rgb565Pixel(2),
                Rgb565Pixel(4),
                Rgb565Pixel(12),
                Rgb565Pixel(14),
                Rgb565Pixel(16),
            ]
        );
    }

    #[test]
    fn scalar_downsample_handles_odd_dimensions_and_padding() {
        let pixels = (0..24).map(Rgb565Pixel).collect::<Vec<_>>();
        let mut output = Vec::new();

        let geometry = downsample_rgb565_2x_with(
            view(&pixels, 5, 3, 8),
            &mut output,
            DownsampleImplementation::Scalar,
        )
        .expect("downsample padded frame");

        assert_eq!(
            geometry,
            DownsampledGeometry {
                width: 3,
                height: 2
            }
        );
        assert_eq!(
            output,
            vec![
                Rgb565Pixel(0),
                Rgb565Pixel(2),
                Rgb565Pixel(4),
                Rgb565Pixel(16),
                Rgb565Pixel(18),
                Rgb565Pixel(20),
            ]
        );
    }

    #[test]
    fn downsample_rejects_invalid_geometry() {
        let pixels = vec![Rgb565Pixel(0); 8];
        let mut output = vec![Rgb565Pixel(9)];

        assert_eq!(
            downsample_rgb565_2x_with(
                view(&pixels, 4, 3, 4),
                &mut output,
                DownsampleImplementation::Scalar,
            ),
            Err("RGB565 source buffer is shorter than its geometry")
        );
    }

    #[test]
    fn compiled_auto_implementation_matches_target_capabilities() {
        #[cfg(all(target_arch = "arm", any(target_feature = "neon", mister_arm_neon)))]
        assert_eq!(compiled_implementation(), DownsampleImplementation::Neon);
        #[cfg(not(all(target_arch = "arm", any(target_feature = "neon", mister_arm_neon))))]
        assert_eq!(compiled_implementation(), DownsampleImplementation::Scalar);
    }

    #[cfg(feature = "bench-tools")]
    #[test]
    fn bench_helpers_are_deterministic_and_use_nearest_rank_percentiles() {
        let first = deterministic_pixels(64);
        let second = deterministic_pixels(64);
        assert_eq!(first, second);
        assert_eq!(checksum_rgb565(&first), checksum_rgb565(&second));

        let values = (1..=20).map(Duration::from_millis).collect::<Vec<_>>();
        assert_eq!(percentile_duration(&values, 50), Duration::from_millis(10));
        assert_eq!(percentile_duration(&values, 95), Duration::from_millis(19));
        assert_eq!(percentile_duration(&values, 99), Duration::from_millis(20));
    }

    #[cfg(all(target_arch = "arm", target_feature = "neon"))]
    #[test]
    fn neon_downsample_matches_scalar_for_pseudo_random_frames() {
        let width = 961usize;
        let height = 541usize;
        let stride = 968usize;
        let mut state = 0x1234_5678u32;
        let pixels = (0..stride * height)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                Rgb565Pixel((state >> 16) as u16)
            })
            .collect::<Vec<_>>();
        let mut scalar = Vec::new();
        let mut neon = Vec::new();

        let scalar_geometry = downsample_rgb565_2x_with(
            view(&pixels, width, height, stride),
            &mut scalar,
            DownsampleImplementation::Scalar,
        )
        .expect("scalar downsample");
        let neon_geometry = downsample_rgb565_2x_with(
            view(&pixels, width, height, stride),
            &mut neon,
            DownsampleImplementation::Neon,
        )
        .expect("NEON downsample");

        assert_eq!(neon_geometry, scalar_geometry);
        assert_eq!(neon, scalar);
    }
}
