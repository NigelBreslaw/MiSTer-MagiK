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
    /// Time each band's thread was runnable while another task held its CPU.
    pub primary_run_delay_us: Option<u64>,
    pub secondary_run_delay_us: Option<u64>,
}
impl FrameWorkTiming {
    fn json(self) -> Value {
        json!({"producer_us":self.producer_us,"primary_us":self.primary_us,"secondary_us":self.secondary_us,
            "wait_us":self.wait_us,"helper_start_delay_us":self.helper_start_delay_us,
            "completion_delivery_us":self.completion_delivery_us,"merge_us":self.merge_us,
            "primary_cpu_us":self.primary_cpu_us,"secondary_cpu_us":self.secondary_cpu_us,
            "split":self.split,
            "primary_run_delay_us":self.primary_run_delay_us,
            "secondary_run_delay_us":self.secondary_run_delay_us})
    }
}

/// Activity when a missed refresh was observed, not a causal attribution.
#[derive(Clone, Copy, Debug, Default)]
pub enum FrameWorkload {
    Card,
    SystemTransition,
    Slint,
    Screensaver,
    #[default]
    Unknown,
}
impl FrameWorkload {
    pub fn label(self) -> &'static str {
        match self {
            Self::Card => "card",
            Self::SystemTransition => "system-transition",
            Self::Slint => "slint",
            Self::Screensaver => "screensaver",
            Self::Unknown => "unknown",
        }
    }
}

/// Host observations in microseconds from the same process-monotonic epoch.
/// The interval can contain earlier frame work; it is not one frame's CPU time.
#[derive(Clone, Copy, Debug, Default)]
pub struct FramePhaseTimeline {
    pub previous_observation_us: u64,
    pub frame_begin_us: u64,
    pub render_start_us: u64,
    pub render_end_us: u64,
    pub custom_draw_start_us: u64,
    pub custom_draw_end_us: u64,
    pub present_start_us: u64,
    pub post_returned_us: u64,
    pub post_request_start_us: Option<u64>,
    pub post_verified_us: Option<u64>,
    pub confirmation_wait_start_us: u64,
    pub active_observed_us: u64,
    pub telemetry_observed_us: u64,
    pub refresh_period_us: u64,
    pub frame_start_phase_us: u64,
    pub present_start_phase_us: u64,
    pub tooling_tick_us: u64,
    pub pre_render_wait_us: u64,
    pub hidden_copy_us: u64,
    pub hidden_publish_us: u64,
    pub latch_request_us: u64,
    pub post_status_us: u64,
    pub completion_poll_us: u64,
    pub previous_active_sequence: u16,
    pub posted_sequence: u16,
    pub post_active_sequence: u16,
    pub post_pending_sequence: u16,
    pub post_pending: bool,
    pub previous_owned_refresh: u32,
    pub owned_refresh_delta: u32,
    pub repeated_refresh_delta: u32,
}
impl FramePhaseTimeline {
    fn json(self) -> Value {
        json!({
            "clock":"process-monotonic-us",
            "previous_observation_us":self.previous_observation_us,
            "frame_begin_us":self.frame_begin_us,
            "render_start_us":self.render_start_us,"render_end_us":self.render_end_us,
            "custom_draw_start_us":self.custom_draw_start_us,"custom_draw_end_us":self.custom_draw_end_us,
            "present_start_us":self.present_start_us,"post_returned_us":self.post_returned_us,
            "post_request_start_us":self.post_request_start_us,"post_verified_us":self.post_verified_us,
            "confirmation_wait_start_us":self.confirmation_wait_start_us,
            "active_observed_us":self.active_observed_us,"telemetry_observed_us":self.telemetry_observed_us,
            "refresh_period_us":self.refresh_period_us,"frame_start_phase_us":self.frame_start_phase_us,
            "present_start_phase_us":self.present_start_phase_us,"tooling_tick_us":self.tooling_tick_us,
            "pre_render_wait_us":self.pre_render_wait_us,"hidden_copy_us":self.hidden_copy_us,
            "hidden_publish_us":self.hidden_publish_us,"latch_request_us":self.latch_request_us,
            "post_status_us":self.post_status_us,"completion_poll_us":self.completion_poll_us,
            "previous_active_sequence":self.previous_active_sequence,"posted_sequence":self.posted_sequence,
            "post_active_sequence":self.post_active_sequence,"post_pending_sequence":self.post_pending_sequence,
            "post_pending":self.post_pending,
            "previous_owned_refresh":self.previous_owned_refresh,"owned_refresh_delta":self.owned_refresh_delta,
            "repeated_refresh_delta":self.repeated_refresh_delta
        })
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
    pub workload: FrameWorkload,
    pub transition_route: &'static str,
    pub transition_renderer: &'static str,
    pub timeline: Option<FramePhaseTimeline>,
}

impl DroppedFrameRecord {
    fn json(self) -> Value {
        json!({"reason":self.reason,"dropped_frames":self.dropped_frames,
            "source_generation":self.source_generation,"source_age_us":self.source_age_us,

            "owned_refresh_observed":self.owned_refresh_observed,"active_sequence":self.active_sequence,
            "ui_render_us":self.ui_render_us,
            "ui_render_scope":self.timeline.map(|_| "before-custom-draw"),
            "work":self.work.map(FrameWorkTiming::json),"workload":self.workload.label(),
            "transition_route":self.transition_route,"transition_renderer":self.transition_renderer,
            "timeline":self.timeline.map(FramePhaseTimeline::json)})
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
    pub screensaver_presentations: u64,
}
#[derive(Default)]
pub struct PresentationMetrics {
    pub frame_timings_us: Vec<[u64; 3]>,
    pub work_timings: Vec<FrameWorkTiming>,
    pub peak_rss_bytes: Option<u64>,
    pub process_cpu_us: Option<u64>,
    pub window_cpu_start_us: Option<u64>,
    pub forced_clock_changes: u64,
    pub card_prepare_max_us: u64,
    pub context: Value,
    pub counters: Counters,
    pub last_render_us: u64,
    pub render_timing_scope: Option<&'static str>,
    pub last_physical_drop_count: Option<u16>,
    pub motion_started_ms: Option<u64>,
    pub window_start: Option<(u64, Counters)>,
    pub window: Option<Value>,
    pub error: Option<String>,
    pub last_card_source_timestamp_us: u64,
    pub last_card_source_generation: u64,
    pub dropped_frame_records: Vec<DroppedFrameRecord>,
    pub last_dropped_frame: Option<DroppedFrameRecord>,
    pub dropped_frame_records_omitted: u64,
    pub dropped_frames_by_workload: [u64; 5],
}
impl PresentationMetrics {
    /// Reserve on begin; drop records perform no allocation or serialisation.
    pub fn record_dropped_frame(&mut self, record: DroppedFrameRecord) {
        self.last_dropped_frame = Some(record);
        if self.window_start.is_some() && self.window.is_none() {
            let index = match record.workload {
                FrameWorkload::Card => 0,
                FrameWorkload::SystemTransition => 1,
                FrameWorkload::Slint => 2,
                FrameWorkload::Screensaver => 3,
                FrameWorkload::Unknown => 4,
            };
            self.dropped_frames_by_workload[index] =
                self.dropped_frames_by_workload[index].saturating_add(record.dropped_frames);
            if self.dropped_frame_records.len() < 64 {
                self.dropped_frame_records.push(record);
            } else {
                self.dropped_frame_records_omitted =
                    self.dropped_frame_records_omitted.saturating_add(1);
            }
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

        window["render_timing_scope"] = json!(self.render_timing_scope);
        window["drop_attribution"] = json!("activity at observation; not proven cause");
        window["dropped_frame_records_omitted"] = json!(self.dropped_frame_records_omitted);
        window["dropped_frames_by_workload"] = json!(
            [
                FrameWorkload::Card,
                FrameWorkload::SystemTransition,
                FrameWorkload::Slint,
                FrameWorkload::Screensaver,
                FrameWorkload::Unknown
            ]
            .into_iter()
            .zip(self.dropped_frames_by_workload)
            .map(|(kind, count)| (kind.label().to_string(), json!(count)))
            .collect::<serde_json::Map<String, Value>>()
        );
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
            "primary_run_delay",
            "secondary_run_delay",
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
                    8 => t.secondary_cpu_us,
                    9 => t.primary_run_delay_us,
                    _ => t.secondary_run_delay_us,
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
        window["context"] = self.context.clone();
        window["peak_rss_bytes"] = json!(self.peak_rss_bytes);
        window["latch_drops"] = json!(c.latch_drops - baseline.latch_drops);
        window["transfer_us_total"] = json!(c.transfer_us - baseline.transfer_us);
        window["owned_vblanks"] = json!(c.owned_vblanks - baseline.owned_vblanks);
        window["presented_vblanks"] = json!(c.presented_vblanks - baseline.presented_vblanks);
        window["refresh_hz_scope"] =
            json!("observed owned refreshes / whole window; not display mode Hz");
        window["refresh_hz"] = json!(
            (c.owned_vblanks - baseline.owned_vblanks) as f64 * 1000.0
                / (end_ms - start_ms).max(1) as f64
        );
        window["process_cpu_percent"] =
            json!(cpu_us.map(|us| us as f64 / ((end_ms - start_ms).max(1) as f64 * 10.0)));
        window["card_fallback_copies"] =
            json!(c.card_fallback_copies - baseline.card_fallback_copies);
        window["screensaver_presentations"] =
            json!(c.screensaver_presentations - baseline.screensaver_presentations);
        window["card_fallback_copy_pixels"] =
            json!(c.card_fallback_copy_pixels - baseline.card_fallback_copy_pixels);
    }
    pub fn json(&self, width: usize, height: usize, elapsed_ms: u64) -> Value {
        json!({"context":self.context,"width":width,"height":height,"elapsed_ms":elapsed_ms,"pid":std::process::id(),
            "sha256":std::env::var("MISTER_MAGIK2_ARTIFACT_SHA256").unwrap_or_default(),
            "presentations":self.counters.presentations,"last_render_us":self.last_render_us,
            "render_timing_scope":self.render_timing_scope,
            "last_dropped_frame_record":self.last_dropped_frame.map(DroppedFrameRecord::json),
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
    fn workload_totals_survive_record_limit_and_frozen_window_preserves_late_evidence() {
        let mut metrics = PresentationMetrics {
            window_start: Some((0, Counters::default())),
            ..Default::default()
        };
        for i in 0..100 {
            if i % 2 == 0 {
                metrics.counters.card_dropped_frames += 1;
            } else {
                metrics.counters.drops += 1;
            }
            metrics.record_dropped_frame(DroppedFrameRecord {
                dropped_frames: 1,
                workload: if i % 2 == 0 {
                    FrameWorkload::Card
                } else {
                    FrameWorkload::SystemTransition
                },
                ui_render_us: 14,
                timeline: Some(FramePhaseTimeline {
                    render_start_us: 100,
                    render_end_us: 114,
                    custom_draw_start_us: 115,
                    custom_draw_end_us: 8_115,
                    post_request_start_us: Some(16_900),
                    post_verified_us: Some(17_100),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }
        metrics.finish_window(1_000, 960, 540, false);
        let frozen = metrics.window.clone().unwrap();
        assert_eq!(frozen["dropped_frames"], 100);
        assert_eq!(frozen["owned_refresh_dropped_frames"], 50);
        assert!(frozen.get("physical_drops").is_none());
        assert!(frozen.get("card_redisplayed_presentations").is_none());
        assert!(frozen["render_timing_scope"].is_null());
        assert_eq!(frozen["dropped_frame_records_omitted"], 36);
        assert_eq!(frozen["dropped_frames_by_workload"]["card"], 50);
        assert_eq!(
            frozen["dropped_frames_by_workload"]["system-transition"],
            50
        );
        assert_eq!(
            frozen["dropped_frame_records"].as_array().unwrap().len(),
            64
        );
        let record = &frozen["dropped_frame_records"][0];
        assert_eq!(record["ui_render_us"], 14);
        assert_eq!(record["timeline"]["custom_draw_end_us"], 8_115);
        assert_eq!(record["timeline"]["post_request_start_us"], 16_900);
        // Late drop evidence stays accessible to cumulative step snapshots, but
        // cannot silently extend the completed device-clock measurement window.
        metrics.record_dropped_frame(DroppedFrameRecord {
            active_sequence: Some(511),
            dropped_frames: 2,
            ..Default::default()
        });
        assert_eq!(metrics.window.as_ref(), Some(&frozen));
        assert_eq!(
            metrics.json(960, 540, 1_100)["last_dropped_frame_record"]["active_sequence"],
            511
        );
    }
}
