//! Counters and device-clock windows. Rendering and latch waiting are distinct.
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameWorkTiming {
    pub producer_us: u64,
    pub primary_us: u64,
    pub secondary_us: u64,
    pub wait_us: u64,
    pub helper_start_delay_us: u64,
    pub completion_delivery_us: u64,
    pub merge_us: u64,
    pub primary_cpu_us: Option<u64>,
    pub secondary_cpu_us: Option<u64>,
    /// First helper column; zero when the producer has no band split.
    pub split: u64,
}
impl FrameWorkTiming {
    fn json(self) -> Value {
        json!({"producer_us":self.producer_us,"primary_us":self.primary_us,"secondary_us":self.secondary_us,
            "wait_us":self.wait_us,"helper_start_delay_us":self.helper_start_delay_us,
            "completion_delivery_us":self.completion_delivery_us,"merge_us":self.merge_us,
            "primary_cpu_us":self.primary_cpu_us,"secondary_cpu_us":self.secondary_cpu_us,
            "split":self.split})
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DroppedFrameRecord {
    pub reason: &'static str,
    pub dropped_frames: u64,
    pub source_generation: u64,
    pub source_age_us: u64,
    pub owned_refresh_observed: Option<u32>,
    pub active_sequence: Option<u16>,
    pub ui_render_us: u64,
    pub work: Option<FrameWorkTiming>,
}

impl DroppedFrameRecord {
    fn json(self) -> Value {
        json!({"reason":self.reason,"dropped_frames":self.dropped_frames,
            "source_generation":self.source_generation,"source_age_us":self.source_age_us,

            "owned_refresh_observed":self.owned_refresh_observed,"active_sequence":self.active_sequence,
            "ui_render_us":self.ui_render_us,"work":self.work.map(FrameWorkTiming::json)})
    }
}

#[derive(Default, Clone)]
pub struct Counters {
    pub owned_vblanks: u64,
    pub presented_vblanks: u64,
    pub latch_drops: u64,
    pub transfer_us: u64,
    pub presentations: u64,
    pub render_us: u64,
    pub render_to_present_us: u64,
    pub posts: u64,
    pub flips: u64,
    pub drops: u64,
    pub rejections: u64,
    pub card_rendered_frames: u64,
    pub card_delivered_frames: u64,
    pub card_dropped_frames: u64,
    pub card_synchronous_presentations: u64,
    pub card_continuous_presentations: u64,
    pub card_producer_total_us: u64,
    pub card_primary_tile_us: u64,
    pub card_secondary_tile_us: u64,
    pub card_secondary_wait_us: u64,
    pub card_hidden_copy_us: u64,
    pub card_source_age_us: u64,
    pub card_chrome_refreshes: u64,
    pub card_prepare_us: u64,
    pub card_fallback_copies: u64,
    pub card_fallback_copy_pixels: u64,
}
#[derive(Default)]
pub struct PresentationMetrics {
    pub frame_timings_us: Vec<[u64; 3]>,
    pub work_timings: Vec<FrameWorkTiming>,
    /// Per delivered card frame: pose sample time to the vblank that showed it.
    /// Its spread is positional judder that dropped-frame counts cannot see.
    pub pose_to_scanout_us: Vec<u64>,
    pub peak_rss_bytes: Option<u64>,
    pub process_cpu_us: Option<u64>,
    pub window_cpu_start_us: Option<u64>,
    pub forced_clock_changes: u64,
    pub card_prepare_max_us: u64,
    pub context: Value,
    pub counters: Counters,
    pub last_render_us: u64,
    pub last_physical_drop_count: Option<u16>,
    pub motion_started_ms: Option<u64>,
    pub window_start: Option<(u64, Counters)>,
    pub window: Option<Value>,
    pub error: Option<String>,
    pub last_card_source_timestamp_us: u64,
    pub last_card_source_generation: u64,
    pub dropped_frame_records: Vec<DroppedFrameRecord>,
}
impl PresentationMetrics {
    /// Reserve on begin; drop records perform no allocation or serialisation.
    pub fn record_dropped_frame(&mut self, record: DroppedFrameRecord) {
        if self.window_start.is_some()
            && self.window.is_none()
            && self.dropped_frame_records.len() < 64
        {
            self.dropped_frame_records.push(record);
        }
    }

    pub fn finish_window(&mut self, end_ms: u64, width: usize, height: usize, instrumented: bool) {
        let (start_ms, baseline) = self.window_start.as_ref().expect("measurement started");
        let c = &self.counters;
        let cpu_us = self
            .process_cpu_us
            .zip(self.window_cpu_start_us)
            .map(|(end, start)| end.saturating_sub(start));
        self.window = Some(
            json!({"start_ms":start_ms,"end_ms":end_ms,"elapsed_ms":end_ms-start_ms,
            "width":width,"height":height,"instrumented":instrumented,
            "forced_clock_changes":self.forced_clock_changes,
            "card_chrome_refreshes":c.card_chrome_refreshes-baseline.card_chrome_refreshes,
            "card_prepare_us":c.card_prepare_us-baseline.card_prepare_us,
            "card_prepare_max_us":self.card_prepare_max_us,
            "presentations":c.presentations-baseline.presentations,"render_us_total":c.render_us-baseline.render_us,
            "render_to_present_us_total":c.render_to_present_us-baseline.render_to_present_us,
            "physical_latch_posts":c.posts-baseline.posts,"physical_latch_flips":c.flips-baseline.flips,
            "dropped_frames":(c.drops-baseline.drops)+(c.card_dropped_frames-baseline.card_dropped_frames),
            "owned_refresh_dropped_frames":c.drops-baseline.drops,"latch_rejections":c.rejections-baseline.rejections,

            "card_rendered_frames":c.card_rendered_frames-baseline.card_rendered_frames,

            "card_delivered_frames":c.card_delivered_frames-baseline.card_delivered_frames,

            "card_synchronous_presentations":c.card_synchronous_presentations-baseline.card_synchronous_presentations,
            "card_continuous_presentations":c.card_continuous_presentations-baseline.card_continuous_presentations,
            "card_producer_total_us":c.card_producer_total_us-baseline.card_producer_total_us,
            "card_primary_tile_us":c.card_primary_tile_us-baseline.card_primary_tile_us,
            "card_secondary_tile_us":c.card_secondary_tile_us-baseline.card_secondary_tile_us,
            "card_secondary_wait_us":c.card_secondary_wait_us-baseline.card_secondary_wait_us,
            "card_hidden_copy_us":c.card_hidden_copy_us-baseline.card_hidden_copy_us,
            "card_source_age_us":c.card_source_age_us-baseline.card_source_age_us,
            "last_card_source_timestamp_us":self.last_card_source_timestamp_us,
            "last_card_source_generation":self.last_card_source_generation,
            "evidence_error":self.error,"drop_baseline_available":self.last_physical_drop_count.is_some()}),
        );
        let window = self.window.as_mut().unwrap();
        window["process_cpu_us"] = json!(cpu_us);
        for (index, name) in ["render", "transfer", "frame_to_present"]
            .into_iter()
            .enumerate()
        {
            let mut samples = self
                .frame_timings_us
                .iter()
                .map(|s| s[index])
                .collect::<Vec<_>>();
            samples.sort_unstable();
            if !samples.is_empty() {
                window[format!("{name}_average_us")] =
                    json!(samples.iter().sum::<u64>() as f64 / samples.len() as f64);
                window[format!("{name}_p99_us")] =
                    json!(samples[(samples.len() * 99 / 100).min(samples.len() - 1)]);
                window[format!("{name}_max_us")] = json!(samples.last());
            }
        }

        window["dropped_frame_records"] = json!(
            self.dropped_frame_records
                .iter()
                .copied()
                .map(DroppedFrameRecord::json)
                .collect::<Vec<_>>()
        );
        for (index, name) in [
            "producer",
            "primary",
            "secondary",
            "helper_wait",
            "helper_start_delay",
            "completion_delivery",
            "merge",
            "primary_cpu",
            "secondary_cpu",
        ]
        .into_iter()
        .enumerate()
        {
            let mut samples = self
                .work_timings
                .iter()
                .filter_map(|t| match index {
                    0 => Some(t.producer_us),
                    1 => Some(t.primary_us),
                    2 => Some(t.secondary_us),
                    3 => Some(t.wait_us),
                    4 => Some(t.helper_start_delay_us),
                    5 => Some(t.completion_delivery_us),
                    6 => Some(t.merge_us),
                    7 => t.primary_cpu_us,
                    _ => t.secondary_cpu_us,
                })
                .collect::<Vec<_>>();
            samples.sort_unstable();
            if !samples.is_empty() {
                window[format!("{name}_average_us")] =
                    json!(samples.iter().sum::<u64>() as f64 / samples.len() as f64);
                window[format!("{name}_p99_us")] =
                    json!(samples[(samples.len() * 99 / 100).min(samples.len() - 1)]);
                window[format!("{name}_max_us")] = json!(samples.last());
            }
        }
        let mut pose = self.pose_to_scanout_us.clone();
        pose.sort_unstable();
        if !pose.is_empty() {
            let at = |permille: usize| pose[(pose.len() * permille / 1000).min(pose.len() - 1)];
            window["card_pose_to_scanout"] = json!({
                "samples": pose.len(),
                "min_us": pose[0],
                "p1_us": at(10),
                "p50_us": at(500),
                "p99_us": at(990),
                "max_us": pose[pose.len() - 1],
            });
        }
        window["context"] = self.context.clone();
        window["peak_rss_bytes"] = json!(self.peak_rss_bytes);
        window["latch_drops"] = json!(c.latch_drops - baseline.latch_drops);
        window["transfer_us_total"] = json!(c.transfer_us - baseline.transfer_us);
        window["owned_vblanks"] = json!(c.owned_vblanks - baseline.owned_vblanks);
        window["presented_vblanks"] = json!(c.presented_vblanks - baseline.presented_vblanks);
        window["refresh_hz"] = json!(
            (c.owned_vblanks - baseline.owned_vblanks) as f64 * 1000.0
                / (end_ms - start_ms).max(1) as f64
        );
        window["process_cpu_percent"] =
            json!(cpu_us.map(|us| us as f64 / ((end_ms - start_ms).max(1) as f64 * 10.0)));
        window["card_fallback_copies"] =
            json!(c.card_fallback_copies - baseline.card_fallback_copies);
        window["card_fallback_copy_pixels"] =
            json!(c.card_fallback_copy_pixels - baseline.card_fallback_copy_pixels);
    }
    pub fn json(&self, width: usize, height: usize, elapsed_ms: u64) -> Value {
        json!({"context":self.context,"width":width,"height":height,"elapsed_ms":elapsed_ms,"pid":std::process::id(),
            "sha256":std::env::var("MISTER_MAGIK2_ARTIFACT_SHA256").unwrap_or_default(),
            "presentations":self.counters.presentations,"last_render_us":self.last_render_us,
            "render_us_total":self.counters.render_us,"render_to_present_us_total":self.counters.render_to_present_us,
            "physical_latch_posts":self.counters.posts,"physical_latch_flips":self.counters.flips,
            "dropped_frames":self.counters.drops+self.counters.card_dropped_frames,"latch_rejections":self.counters.rejections,
            "card_rendered_frames":self.counters.card_rendered_frames,

            "card_delivered_frames":self.counters.card_delivered_frames,
            "card_dropped_frames":self.counters.card_dropped_frames,

            "card_producer_total_us":self.counters.card_producer_total_us,
            "card_primary_tile_us":self.counters.card_primary_tile_us,
            "card_secondary_tile_us":self.counters.card_secondary_tile_us,
            "card_secondary_wait_us":self.counters.card_secondary_wait_us,
            "card_hidden_copy_us":self.counters.card_hidden_copy_us,
            "card_source_age_us":self.counters.card_source_age_us,
            "last_card_source_timestamp_us":self.last_card_source_timestamp_us,
            "last_card_source_generation":self.last_card_source_generation,


            "motion_started_ms":self.motion_started_ms,"window":self.window,"evidence_error":self.error})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_window_reports_process_delta_and_preserves_unavailable() {
        let mut metrics = PresentationMetrics {
            window_start: Some((2000, Counters::default())),
            ..Default::default()
        };
        metrics.finish_window(7000, 960, 540, true);
        assert!(metrics.window.as_ref().unwrap()["process_cpu_us"].is_null());
        metrics.window_cpu_start_us = Some(1_000_000);
        metrics.process_cpu_us = Some(7_000_000);
        metrics.counters.card_fallback_copies = 300;
        metrics.counters.card_fallback_copy_pixels = 300 * 518_400;
        metrics.finish_window(7000, 960, 540, true);
        let window = metrics.window.unwrap();
        assert_eq!(window["process_cpu_us"], 6_000_000);
        assert_eq!(window["process_cpu_percent"], 120.0);
        assert_eq!(window["card_fallback_copy_pixels"], 155_520_000);
    }

    #[test]
    fn window_excludes_warmup_and_keeps_drop_and_rejection_evidence() {
        let mut metrics = PresentationMetrics::default();
        metrics.counters.presentations = 100;
        metrics.counters.drops = 2;
        metrics.window_start = Some((2000, metrics.counters.clone()));
        metrics.counters.presentations = 400;
        metrics.counters.drops = 3;
        metrics.counters.rejections = 1;
        metrics.finish_window(7000, 960, 540, false);
        let window = metrics.window.unwrap();
        assert_eq!(window["presentations"], 300);
        assert_eq!(window["dropped_frames"], 1);
        assert_eq!(window["latch_rejections"], 1);
        assert_eq!(window["elapsed_ms"], 5000);
    }

    #[test]
    fn card_window_reports_delivered_dropped_and_producer_work() {
        let mut metrics = PresentationMetrics::default();
        metrics.counters.presentations = 10;
        metrics.counters.card_delivered_frames = 7;
        metrics.counters.card_dropped_frames = 3;
        metrics.counters.card_producer_total_us = 700;
        metrics.counters.card_hidden_copy_us = 90;
        metrics.window_start = Some((1_000, metrics.counters.clone()));
        metrics.counters.presentations += 300;
        metrics.counters.card_delivered_frames += 240;
        metrics.counters.card_dropped_frames += 60;
        metrics.counters.card_producer_total_us += 24_000;
        metrics.counters.card_hidden_copy_us += 2_700;
        metrics.last_card_source_timestamp_us = 5_990_000;
        metrics.last_card_source_generation = 42;

        metrics.finish_window(6_000, 960, 540, true);
        let window = metrics.window.unwrap();
        assert_eq!(window["card_delivered_frames"], 240);
        assert_eq!(window["dropped_frames"], 60);
        assert_eq!(
            window["card_delivered_frames"].as_u64().unwrap()
                + window["dropped_frames"].as_u64().unwrap(),
            window["presentations"].as_u64().unwrap()
        );
        assert_eq!(window["card_producer_total_us"], 24_000);
        assert_eq!(window["card_hidden_copy_us"], 2_700);
        assert_eq!(window["last_card_source_generation"], 42);
    }

    #[test]
    fn dropped_frames_cover_distinct_failed_refreshes_and_bound_evidence() {
        let mut metrics = PresentationMetrics::default();
        metrics.window_start = Some((0, Counters::default()));
        metrics.dropped_frame_records.reserve(64);
        metrics.counters.card_dropped_frames = 60;
        metrics.counters.drops = 5;
        for _ in 0..100 {
            metrics.record_dropped_frame(DroppedFrameRecord {
                reason: "producer not complete at readiness check",
                dropped_frames: 1,
                ..Default::default()
            });
        }
        assert_eq!(metrics.dropped_frame_records.len(), 64);
        metrics.finish_window(1_000, 960, 540, false);
        let window = metrics.window.unwrap();
        assert_eq!(window["dropped_frames"], 65);
        assert_eq!(window["owned_refresh_dropped_frames"], 5);
        assert_eq!(
            window["dropped_frame_records"].as_array().unwrap().len(),
            64
        );
        assert!(window.get("physical_drops").is_none());
        assert!(window.get("card_redisplayed_presentations").is_none());
    }
}
