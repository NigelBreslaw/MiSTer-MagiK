//! Counters and device-clock windows. Rendering and latch waiting are distinct.
use serde_json::{Value, json};
#[derive(Default, Clone)]
pub struct Counters {
    pub presentations: u64,
    pub render_us: u64,
    pub render_to_present_us: u64,
    pub posts: u64,
    pub flips: u64,
    pub drops: u64,
    pub rejections: u64,
    pub card_submitted: u64,
    pub card_completed: u64,
    pub card_superseded: u64,
    pub card_stale: u64,
    pub card_unique_presentations: u64,
    pub card_redisplayed_presentations: u64,
    pub card_synchronous_presentations: u64,
    pub card_producer_total_us: u64,
    pub card_primary_tile_us: u64,
    pub card_secondary_tile_us: u64,
    pub card_secondary_wait_us: u64,
    pub card_composition_us: u64,
    pub card_hidden_copy_us: u64,
    pub card_source_age_us: u64,
}
#[derive(Default)]
pub struct PresentationMetrics {
    pub forced_clock_changes: u64,
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
}
impl PresentationMetrics {
    pub fn finish_window(&mut self, end_ms: u64, width: usize, height: usize, instrumented: bool) {
        let (start_ms, baseline) = self.window_start.as_ref().expect("measurement started");
        let c = &self.counters;
        self.window = Some(
            json!({"start_ms":start_ms,"end_ms":end_ms,"elapsed_ms":end_ms-start_ms,
            "width":width,"height":height,"instrumented":instrumented,
            "forced_clock_changes":self.forced_clock_changes,
            "presentations":c.presentations-baseline.presentations,"render_us_total":c.render_us-baseline.render_us,
            "render_to_present_us_total":c.render_to_present_us-baseline.render_to_present_us,
            "physical_latch_posts":c.posts-baseline.posts,"physical_latch_flips":c.flips-baseline.flips,
            "physical_drops":c.drops-baseline.drops,"latch_rejections":c.rejections-baseline.rejections,
            "card_submitted":c.card_submitted-baseline.card_submitted,
            "card_completed":c.card_completed-baseline.card_completed,
            "card_superseded":c.card_superseded-baseline.card_superseded,
            "card_stale":c.card_stale-baseline.card_stale,
            "card_unique_presentations":c.card_unique_presentations-baseline.card_unique_presentations,
            "card_redisplayed_presentations":c.card_redisplayed_presentations-baseline.card_redisplayed_presentations,
            "card_synchronous_presentations":c.card_synchronous_presentations-baseline.card_synchronous_presentations,
            "card_producer_total_us":c.card_producer_total_us-baseline.card_producer_total_us,
            "card_primary_tile_us":c.card_primary_tile_us-baseline.card_primary_tile_us,
            "card_secondary_tile_us":c.card_secondary_tile_us-baseline.card_secondary_tile_us,
            "card_secondary_wait_us":c.card_secondary_wait_us-baseline.card_secondary_wait_us,
            "card_composition_us":c.card_composition_us-baseline.card_composition_us,
            "card_hidden_copy_us":c.card_hidden_copy_us-baseline.card_hidden_copy_us,
            "card_source_age_us":c.card_source_age_us-baseline.card_source_age_us,
            "last_card_source_timestamp_us":self.last_card_source_timestamp_us,
            "last_card_source_generation":self.last_card_source_generation,
            "evidence_error":self.error,"drop_baseline_available":self.last_physical_drop_count.is_some()}),
        );
    }
    pub fn json(&self, width: usize, height: usize, elapsed_ms: u64) -> Value {
        json!({"context":self.context,"width":width,"height":height,"elapsed_ms":elapsed_ms,"pid":std::process::id(),
            "sha256":std::env::var("MISTER_MAGIK2_ARTIFACT_SHA256").unwrap_or_default(),
            "presentations":self.counters.presentations,"last_render_us":self.last_render_us,
            "render_us_total":self.counters.render_us,"render_to_present_us_total":self.counters.render_to_present_us,
            "physical_latch_posts":self.counters.posts,"physical_latch_flips":self.counters.flips,
            "physical_drops":self.counters.drops,"latch_rejections":self.counters.rejections,
            "card_submitted":self.counters.card_submitted,"card_completed":self.counters.card_completed,
            "card_superseded":self.counters.card_superseded,"card_stale":self.counters.card_stale,
            "card_unique_presentations":self.counters.card_unique_presentations,
            "card_redisplayed_presentations":self.counters.card_redisplayed_presentations,
            "card_producer_total_us":self.counters.card_producer_total_us,
            "card_primary_tile_us":self.counters.card_primary_tile_us,
            "card_secondary_tile_us":self.counters.card_secondary_tile_us,
            "card_secondary_wait_us":self.counters.card_secondary_wait_us,
            "card_composition_us":self.counters.card_composition_us,
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
        assert_eq!(window["physical_drops"], 1);
        assert_eq!(window["latch_rejections"], 1);
        assert_eq!(window["elapsed_ms"], 5000);
    }

    #[test]
    fn card_window_separates_unique_redisplayed_producer_and_copy_work() {
        let mut metrics = PresentationMetrics::default();
        metrics.counters.presentations = 10;
        metrics.counters.card_unique_presentations = 7;
        metrics.counters.card_redisplayed_presentations = 3;
        metrics.counters.card_producer_total_us = 700;
        metrics.counters.card_hidden_copy_us = 90;
        metrics.window_start = Some((1_000, metrics.counters.clone()));
        metrics.counters.presentations += 300;
        metrics.counters.card_unique_presentations += 240;
        metrics.counters.card_redisplayed_presentations += 60;
        metrics.counters.card_producer_total_us += 24_000;
        metrics.counters.card_hidden_copy_us += 2_700;
        metrics.last_card_source_timestamp_us = 5_990_000;
        metrics.last_card_source_generation = 42;

        metrics.finish_window(6_000, 960, 540, true);
        let window = metrics.window.unwrap();
        assert_eq!(window["card_unique_presentations"], 240);
        assert_eq!(window["card_redisplayed_presentations"], 60);
        assert_eq!(
            window["card_unique_presentations"].as_u64().unwrap()
                + window["card_redisplayed_presentations"].as_u64().unwrap(),
            window["presentations"].as_u64().unwrap()
        );
        assert_eq!(window["card_producer_total_us"], 24_000);
        assert_eq!(window["card_hidden_copy_us"], 2_700);
        assert_eq!(window["last_card_source_generation"], 42);
    }
}
