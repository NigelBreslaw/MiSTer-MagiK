// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Stable ASCII title ordering used by discovery and catalog publication.
pub fn sort_ascii_titles<T>(
    rows: &mut [T],
    mut title: impl FnMut(&T) -> &str,
    mut tie: impl FnMut(&T) -> &str,
) {
    rows.sort_by_cached_key(|row| (title(row).to_ascii_lowercase(), tie(row).to_owned()));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_keys_preserve_ascii_ties_stability_and_whole_rows() {
        for count in [0, 1, 1_000, 10_000, 50_000] {
            let mut rows: Vec<_> = (0..count)
                .map(|i| {
                    (
                        format!(
                            "{} {:03}",
                            ["ALPHA", "alpha", "Éclair", "éclair", "Σ", "σ"][i % 6],
                            i % 97
                        ),
                        format!("key-{}", i % 19),
                        i,
                    )
                })
                .collect();
            let mut state = 0x24b712cau64;
            for i in (1..rows.len()).rev() {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                rows.swap(i, state as usize % (i + 1));
            }
            let mut expected = rows.clone();
            expected.sort_by(|a, b| {
                a.0.to_ascii_lowercase()
                    .cmp(&b.0.to_ascii_lowercase())
                    .then_with(|| a.1.cmp(&b.1))
            });
            let mut visits = 0;
            sort_ascii_titles(
                &mut rows,
                |row| {
                    visits += 1;
                    &row.0
                },
                |row| &row.1,
            );
            assert_eq!(rows, expected);
            if count > 1 {
                assert_eq!(visits, count);
            } else {
                assert!(visits <= count);
            }
        }
    }
}
