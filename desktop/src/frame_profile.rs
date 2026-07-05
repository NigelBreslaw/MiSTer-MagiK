use std::collections::HashMap;

pub const FRAME_BUDGET_US: u64 = 16_667;

const PHASES: &[(&str, &str)] = &[
    ("prepare_us", "prepare"),
    ("anim_us", "anim"),
    ("slint_render_us", "slint-render"),
    ("custom_draw_us", "custom-draw"),
    ("vsync_us", "vsync-wait"),
    ("cached_present_us", "cached-present"),
    ("arcade_list_present_us", "arcade-list-present"),
    ("video_decode_us", "video-decode"),
    ("video_scale_us", "video-scale"),
    ("video_recv_us", "video-recv"),
    ("video_image_us", "video-image"),
    ("video_blit_us", "video-blit"),
    ("audio_decode_us", "audio-decode"),
    ("audio_resample_us", "audio-resample"),
    ("audio_write_us", "audio-write"),
];

const HISTOGRAM_BUCKETS: &[(u64, u64, &str)] = &[
    (0, 100, "[0,100us)"),
    (100, 500, "[100,500us)"),
    (500, 1_000, "[0.5,1ms)"),
    (1_000, 2_000, "[1,2ms)"),
    (2_000, 5_000, "[2,5ms)"),
    (5_000, 10_000, "[5,10ms)"),
    (10_000, 15_000, "[10,15ms)"),
    (15_000, 17_000, "[15,17ms)"),
    (17_000, 30_000, "[17,30ms)"),
    (30_000, u64::MAX, "[30ms,+)"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameProfile {
    pub rows: Vec<FrameProfileRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameProfileRow {
    pub frame: u64,
    pub wall_us: u64,
    pub phases_us: u64,
    pub rows: u64,
    pub present_rect: Option<PresentRect>,
    pub present_pixels: u64,
    pub present_bytes: u64,
    pub dominant: String,
    values: HashMap<String, u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentRect {
    pub x0: u64,
    pub y0: u64,
    pub x1: u64,
    pub y1: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhaseSegment {
    pub key: String,
    pub label: String,
    pub us: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameBar {
    pub frame: u64,
    pub wall_us: u64,
    pub over_budget: bool,
    pub segments: Vec<PhaseSegment>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhaseStats {
    pub key: String,
    pub label: String,
    pub avg: u64,
    pub p50: u64,
    pub p95: u64,
    pub p99: u64,
    pub max: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlowFrame {
    pub frame: u64,
    pub wall_us: u64,
    pub dominant: String,
    pub rect: Option<PresentRect>,
    pub present_pixels: u64,
    pub present_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistogramBucket {
    pub label: String,
    pub count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeatmapCell {
    pub x: u32,
    pub y: u32,
    pub hits: u64,
}

impl FrameProfile {
    pub fn parse_tsv(text: &str) -> Result<Self, String> {
        let mut lines = text.lines().filter(|line| !line.trim().is_empty());
        let header = lines
            .next()
            .ok_or_else(|| "frame profile TSV is empty".to_string())?;
        let columns = header.split('\t').collect::<Vec<_>>();
        let mut rows = Vec::new();
        for (line_index, line) in lines.enumerate() {
            let cells = line.split('\t').collect::<Vec<_>>();
            rows.push(parse_row(&columns, &cells).map_err(|err| {
                format!("frame profile row {}: {err}", line_index.saturating_add(2))
            })?);
        }
        Ok(Self { rows })
    }

    pub fn frame_bars(&self, limit: usize) -> Vec<FrameBar> {
        self.rows
            .iter()
            .take(limit)
            .map(|row| FrameBar {
                frame: row.frame,
                wall_us: row.wall_us,
                over_budget: row.wall_us >= FRAME_BUDGET_US,
                segments: PHASES
                    .iter()
                    .filter_map(|(key, label)| {
                        let us = row.value(key);
                        (us > 0).then(|| PhaseSegment {
                            key: (*key).to_string(),
                            label: (*label).to_string(),
                            us,
                        })
                    })
                    .collect(),
            })
            .collect()
    }

    pub fn phase_stats(&self) -> Vec<PhaseStats> {
        let mut keys = vec![("wall_us", "wall")];
        keys.extend_from_slice(PHASES);
        keys.into_iter()
            .filter_map(|(key, label)| {
                let mut values = self
                    .rows
                    .iter()
                    .map(|row| row.value(key))
                    .collect::<Vec<_>>();
                if values.is_empty() {
                    return None;
                }
                let avg = values.iter().sum::<u64>() / values.len() as u64;
                values.sort_unstable();
                Some(PhaseStats {
                    key: key.to_string(),
                    label: label.to_string(),
                    avg,
                    p50: percentile(&values, 50),
                    p95: percentile(&values, 95),
                    p99: percentile(&values, 99),
                    max: values.last().copied().unwrap_or(0),
                })
            })
            .collect()
    }

    pub fn slow_frames(&self, limit: usize, threshold_us: u64) -> Vec<SlowFrame> {
        let mut rows = self
            .rows
            .iter()
            .filter(|row| row.wall_us >= threshold_us)
            .collect::<Vec<_>>();
        rows.sort_by_key(|row| std::cmp::Reverse(row.wall_us));
        rows.into_iter()
            .take(limit)
            .map(|row| SlowFrame {
                frame: row.frame,
                wall_us: row.wall_us,
                dominant: row.dominant.clone(),
                rect: row.present_rect,
                present_pixels: row.present_pixels,
                present_bytes: row.present_bytes,
            })
            .collect()
    }

    pub fn histogram(&self, key: &str) -> Vec<HistogramBucket> {
        HISTOGRAM_BUCKETS
            .iter()
            .filter_map(|(low, high, label)| {
                let count = self
                    .rows
                    .iter()
                    .filter(|row| {
                        let value = row.value(key);
                        value >= *low && value < *high
                    })
                    .count() as u64;
                (count > 0).then(|| HistogramBucket {
                    label: (*label).to_string(),
                    count,
                })
            })
            .collect()
    }

    pub fn heatmap(&self, cols: u32, rows: u32) -> Vec<HeatmapCell> {
        if cols == 0 || rows == 0 {
            return Vec::new();
        }
        let surface_w = self.rows.iter().map(|row| row.present_rect.map_or(0, |r| r.x1)).max().unwrap_or(0).max(1);
        let surface_h = self.rows.iter().map(|row| row.present_rect.map_or(0, |r| r.y1)).max().unwrap_or(0).max(1);
        let len = cols as usize * rows as usize;
        let mut grid = vec![0_u64; len];
        for rect in self.rows.iter().filter_map(|row| row.present_rect) {
            add_rect_to_grid(&mut grid, cols, rows, surface_w, surface_h, rect);
        }
        grid.into_iter()
            .enumerate()
            .filter_map(|(index, hits)| {
                (hits > 0).then(|| HeatmapCell {
                    x: index as u32 % cols,
                    y: index as u32 / cols,
                    hits,
                })
            })
            .collect()
    }
}

impl FrameProfileRow {
    pub fn value(&self, key: &str) -> u64 {
        if key == "arcade_list_present_us" {
            return self
                .values
                .get(key)
                .or_else(|| self.values.get("overlay_present_us"))
                .copied()
                .unwrap_or(0);
        }
        self.values.get(key).copied().unwrap_or(0)
    }
}

fn parse_row(columns: &[&str], cells: &[&str]) -> Result<FrameProfileRow, String> {
    let values = columns
        .iter()
        .enumerate()
        .filter_map(|(index, key)| cells.get(index).map(|value| ((*key).to_string(), int_value(value))))
        .collect::<HashMap<_, _>>();
    let frame = value_from(&values, "frame");
    let wall_us = value_from(&values, "wall_us");
    let phases_us = value_from(&values, "phases_us");
    let x0 = value_from(&values, "present_x0");
    let y0 = value_from(&values, "present_y0");
    let x1 = value_from(&values, "present_x1");
    let y1 = value_from(&values, "present_y1");
    let present_rect = (x1 > x0 && y1 > y0).then_some(PresentRect { x0, y0, x1, y1 });
    let dominant = columns
        .iter()
        .position(|key| *key == "dominant")
        .and_then(|index| cells.get(index))
        .filter(|value| !value.is_empty())
        .map(|value| (*value).to_string())
        .unwrap_or_else(|| dominant_phase(&values).to_string());
    Ok(FrameProfileRow {
        frame,
        wall_us,
        phases_us,
        rows: value_from(&values, "rows"),
        present_rect,
        present_pixels: value_from(&values, "present_pixels"),
        present_bytes: value_from(&values, "present_bytes"),
        dominant,
        values,
    })
}

fn int_value(value: &str) -> u64 {
    value.parse::<u64>().unwrap_or_else(|_| {
        value
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map(|value| value as u64)
            .unwrap_or(0)
    })
}

fn value_from(values: &HashMap<String, u64>, key: &str) -> u64 {
    values.get(key).copied().unwrap_or(0)
}

fn dominant_phase(values: &HashMap<String, u64>) -> &'static str {
    PHASES
        .iter()
        .max_by_key(|(key, _)| value_from(values, key))
        .map(|(_, label)| *label)
        .unwrap_or("unknown")
}

fn percentile(sorted_values: &[u64], pct: u64) -> u64 {
    if sorted_values.is_empty() {
        return 0;
    }
    let idx = ((sorted_values.len() - 1) as u64 * pct + 50) / 100;
    sorted_values[idx.min((sorted_values.len() - 1) as u64) as usize]
}

fn add_rect_to_grid(
    grid: &mut [u64],
    cols: u32,
    rows: u32,
    surface_w: u64,
    surface_h: u64,
    rect: PresentRect,
) {
    if rect.x1 <= rect.x0 || rect.y1 <= rect.y0 {
        return;
    }
    let gx0 = ((rect.x0 * cols as u64) / surface_w).min(cols.saturating_sub(1) as u64);
    let gx1 = (((rect.x1 - 1) * cols as u64) / surface_w).min(cols.saturating_sub(1) as u64);
    let gy0 = ((rect.y0 * rows as u64) / surface_h).min(rows.saturating_sub(1) as u64);
    let gy1 = (((rect.y1 - 1) * rows as u64) / surface_h).min(rows.saturating_sub(1) as u64);
    for gy in gy0..=gy1 {
        for gx in gx0..=gx1 {
            let index = gy as usize * cols as usize + gx as usize;
            if let Some(cell) = grid.get_mut(index) {
                *cell += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "frame\tprepare_us\tanim_us\tslint_render_us\tcustom_draw_us\tvsync_us\tfb_present_us\tcached_present_us\toverlay_present_us\tphases_us\twall_us\trows\tpresent_x0\tpresent_y0\tpresent_x1\tpresent_y1\tpresent_pixels\tpresent_bytes\tdominant\n\
0\t100\t0\t300\t400\t1000\t200\t150\t25\t2100\t3000\t10\t0\t0\t960\t540\t518400\t1036800\tslint-render\n\
1\t50\t0\t200\t100\t1000\t150\t120\t30\t1630\t17000\t2\t10\t20\t110\t70\t5000\t10000\tcustom-draw\n\
2\t25\t0\t100\t50\t800\t75\t60\t15\t1125\t1200\t0\t0\t0\t0\t0\t0\t0\t\n";

    #[test]
    fn parses_tsv_and_overlay_present_alias() {
        let profile = FrameProfile::parse_tsv(SAMPLE).expect("profile");

        assert_eq!(profile.rows.len(), 3);
        assert_eq!(profile.rows[1].frame, 1);
        assert_eq!(profile.rows[1].value("arcade_list_present_us"), 30);
        assert_eq!(
            profile.rows[1].present_rect,
            Some(PresentRect {
                x0: 10,
                y0: 20,
                x1: 110,
                y1: 70
            })
        );
    }

    #[test]
    fn frame_bars_mark_budget_and_segments() {
        let profile = FrameProfile::parse_tsv(SAMPLE).expect("profile");
        let bars = profile.frame_bars(2);

        assert!(!bars[0].over_budget);
        assert!(bars[1].over_budget);
        assert!(bars[1]
            .segments
            .iter()
            .any(|segment| segment.key == "arcade_list_present_us" && segment.us == 30));
    }

    #[test]
    fn stats_and_slow_frames_are_ordered() {
        let profile = FrameProfile::parse_tsv(SAMPLE).expect("profile");
        let stats = profile.phase_stats();
        let wall = stats.iter().find(|stat| stat.key == "wall_us").unwrap();

        assert_eq!(wall.max, 17_000);
        assert_eq!(wall.p95, 17_000);
        assert_eq!(profile.slow_frames(4, FRAME_BUDGET_US)[0].frame, 1);
    }

    #[test]
    fn histogram_uses_expected_buckets() {
        let profile = FrameProfile::parse_tsv(SAMPLE).expect("profile");
        let histogram = profile.histogram("wall_us");

        assert!(histogram
            .iter()
            .any(|bucket| bucket.label == "[1,2ms)" && bucket.count == 1));
        assert!(histogram
            .iter()
            .any(|bucket| bucket.label == "[17,30ms)" && bucket.count == 1));
    }

    #[test]
    fn heatmap_buckets_present_rects() {
        let profile = FrameProfile::parse_tsv(SAMPLE).expect("profile");
        let heatmap = profile.heatmap(4, 2);

        assert!(heatmap.iter().any(|cell| cell.x == 0 && cell.y == 0));
        assert!(heatmap.iter().any(|cell| cell.x == 3 && cell.y == 1));
        assert!(heatmap.iter().any(|cell| cell.hits >= 2));
    }
}
