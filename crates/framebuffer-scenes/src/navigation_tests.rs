// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

fn geometry() -> NavigationTransitionGeometry {
    NavigationTransitionGeometry {
        source_card: NavigationTransitionRect {
            x: 18,
            y: 74,
            width: 219,
            height: 448,
        },
        source_label: NavigationTransitionRect {
            x: 60,
            y: 260,
            width: 135,
            height: 16,
        },
        destination_title: NavigationTransitionRect {
            x: 18,
            y: 18,
            width: 200,
            height: 24,
        },
        ..NavigationTransitionGeometry::default()
    }
}

fn request() -> NavigationTransitionRequest {
    NavigationTransitionRequest::new(
        NavigationTransitionEdge::HomeToConsoles,
        NavigationTransitionDirection::Forward,
        geometry(),
    )
}

fn system_request(direction: NavigationTransitionDirection) -> NavigationTransitionRequest {
    NavigationTransitionRequest::new(
        NavigationTransitionEdge::ConsolesToSystem,
        direction,
        NavigationTransitionGeometry {
            source_card: NavigationTransitionRect {
                x: 2,
                y: 15,
                width: 20,
                height: 80,
            },
            source_label: NavigationTransitionRect {
                x: 4,
                y: 46,
                width: 16,
                height: 6,
            },
            source_detail: NavigationTransitionRect {
                x: 4,
                y: 53,
                width: 16,
                height: 4,
            },
            destination_title: NavigationTransitionRect {
                x: 2,
                y: 2,
                width: 30,
                height: 8,
            },
            destination_detail: NavigationTransitionRect {
                x: 2,
                y: 11,
                width: 30,
                height: 4,
            },
            destination_list: NavigationTransitionRect {
                x: 8,
                y: 18,
                width: 47,
                height: 76,
            },
            destination_selected_row: NavigationTransitionRect {
                x: 8,
                y: 46,
                width: 47,
                height: 6,
            },
            destination_preview: NavigationTransitionRect {
                x: 58,
                y: 18,
                width: 38,
                height: 76,
            },
            destination_footer: NavigationTransitionRect {
                x: 8,
                y: 95,
                width: 47,
                height: 5,
            },
            ..NavigationTransitionGeometry::default()
        },
    )
}

#[test]
fn settings_page_push_settles_to_exact_snapshots_in_both_directions() {
    let width = 16;
    let height = 3;
    let source = vec![Rgb565Pixel(0xf800); width * height];
    let destination = vec![Rgb565Pixel(0x07e0); width * height];
    let mut buffers = NavigationTransitionBuffers::new(width, height);
    buffers.begin_capture();
    buffers.capture_source(&source).unwrap();
    buffers.capture_destination(&destination).unwrap();

    for direction in [
        NavigationTransitionDirection::Forward,
        NavigationTransitionDirection::Reverse,
    ] {
        let request = NavigationTransitionRequest::settings_page(direction);
        render_settings_page_push(
            &mut buffers,
            request,
            NavigationTransitionFrame {
                progress_q16: 0,
                ..NavigationTransitionFrame::default()
            },
        )
        .unwrap();
        assert_eq!(buffers.working(), source);

        render_settings_page_push(
            &mut buffers,
            request,
            NavigationTransitionFrame {
                progress_q16: PROGRESS_MAX,
                ..NavigationTransitionFrame::default()
            },
        )
        .unwrap();
        assert_eq!(buffers.working(), destination);
    }
}

#[test]
fn portrait_settings_page_push_stays_horizontal_in_both_directions() {
    let width = 8;
    let height = 16;
    let source = (0..width * height)
        .map(|index| Rgb565Pixel(0x1000 + index as u16))
        .collect::<Vec<_>>();
    let destination = (0..width * height)
        .map(|index| Rgb565Pixel(0x2000 + index as u16))
        .collect::<Vec<_>>();
    let mut buffers = NavigationTransitionBuffers::new(width, height);
    buffers.begin_capture();
    buffers.capture_source(&source).unwrap();
    buffers.capture_destination(&destination).unwrap();
    let progress_q16 = PROGRESS_MAX / 2;
    let travel_q16 = spring_ease_q16(progress_q16) as usize;
    let source_travel = width / SETTINGS_PAGE_SOURCE_TRAVEL_DIVISOR as usize;
    let row = 5;

    render_settings_page_push(
        &mut buffers,
        NavigationTransitionRequest::settings_page(NavigationTransitionDirection::Forward),
        NavigationTransitionFrame {
            progress_q16,
            ..NavigationTransitionFrame::default()
        },
    )
    .unwrap();
    let forward_source_columns = source_travel * travel_q16 / PROGRESS_MAX as usize;
    let forward_destination_x = width - width * travel_q16 / PROGRESS_MAX as usize;
    assert!(forward_source_columns > 0);
    assert!(forward_destination_x < width);
    assert_eq!(
        buffers.working()[row * width],
        source[row * width + forward_source_columns]
    );
    assert_eq!(
        buffers.working()[row * width + forward_destination_x],
        destination[row * width]
    );

    render_settings_page_push(
        &mut buffers,
        NavigationTransitionRequest::settings_page(NavigationTransitionDirection::Reverse),
        NavigationTransitionFrame {
            progress_q16,
            ..NavigationTransitionFrame::default()
        },
    )
    .unwrap();
    let reverse_destination_columns =
        source_travel - source_travel * travel_q16 / PROGRESS_MAX as usize;
    let reverse_source_x = width * travel_q16 / PROGRESS_MAX as usize;
    assert!(reverse_destination_columns > 0);
    assert!(reverse_source_x > 0);
    assert_eq!(
        buffers.working()[row * width],
        destination[row * width + reverse_destination_columns]
    );
    assert_eq!(
        buffers.working()[row * width + reverse_source_x],
        source[row * width]
    );
}

#[test]
fn failed_recap_does_not_expose_stale_snapshot() {
    let mut buffers = NavigationTransitionBuffers::new(4, 3);
    let pixels = vec![Rgb565Pixel(0x1234); 12];
    buffers.capture_source(&pixels).unwrap();
    buffers.capture_destination(&pixels).unwrap();

    assert_eq!(
        buffers.capture_source(&pixels[..11]),
        Err(NavigationTransitionFailure::SnapshotSizeMismatch)
    );
    assert!(!buffers.source_ready());
    assert_eq!(buffers.source(), None);
    assert!(buffers.destination_ready());

    assert_eq!(
        buffers.capture_destination(&pixels[..11]),
        Err(NavigationTransitionFailure::SnapshotSizeMismatch)
    );
    assert!(!buffers.destination_ready());
    assert_eq!(buffers.destination(), None);
}

#[test]
fn buffers_reuse_storage_without_clearing_live_snapshots() {
    let mut buffers = NavigationTransitionBuffers::new(4, 3);
    let pixels = (0..12)
        .map(|value| Rgb565Pixel(value as u16))
        .collect::<Vec<_>>();
    buffers.capture_source(&pixels).unwrap();
    buffers.capture_destination(&pixels).unwrap();

    assert_eq!(buffers.source(), Some(pixels.as_slice()));
    assert_eq!(buffers.destination(), Some(pixels.as_slice()));

    let working_ptr = buffers.working().as_ptr();
    buffers.resize(4, 3);
    assert_eq!(buffers.working().as_ptr(), working_ptr);
    assert!(buffers.source_ready());
    assert!(buffers.destination_ready());

    buffers.begin_capture();
    assert!(!buffers.source_ready());
    assert!(!buffers.destination_ready());
}

#[test]
fn zero_sized_buffers_do_not_allocate_storage() {
    let buffers = NavigationTransitionBuffers::new(0, 0);
    assert!(buffers.source.is_empty());
    assert!(buffers.destination.is_empty());
    assert!(buffers.working.is_empty());
    assert!(buffers.scale_source_x.is_empty());
    assert!(buffers.scale_source_y.is_empty());
    assert!(buffers.scale_excluded_x.is_empty());
    assert!(buffers.scale_dither_x.is_empty());
}

#[test]
fn settings_page_push_moves_only_horizontally_with_clipped_row_copies() {
    let width = 8;
    let height = 2;
    let source: Vec<_> = (0..width * height)
        .map(|index| Rgb565Pixel(index as u16))
        .collect();
    let mut output = vec![Rgb565Pixel(0xffff); width * height];

    assert_eq!(
        blit_snapshot_x(&mut output, &source, width, height, -3),
        ((width - 3) * height) as u64
    );
    assert_eq!(&output[..width - 3], &source[3..width]);
    assert_eq!(
        &output[width..width + width - 3],
        &source[width + 3..width * 2]
    );

    output.fill(Rgb565Pixel(0xffff));
    assert_eq!(
        blit_snapshot_x(&mut output, &source, width, height, 3),
        ((width - 3) * height) as u64
    );
    assert_eq!(&output[3..width], &source[..width - 3]);
    assert_eq!(&output[width + 3..width * 2], &source[width..width * 2 - 3]);
}

#[test]
fn settings_page_push_moves_contiguous_rows_in_physical_portrait_space() {
    let width = 4;
    let height = 8;
    let source = (0..width * height)
        .map(|index| Rgb565Pixel(0x1000 + index as u16))
        .collect::<Vec<_>>();
    let destination = (0..width * height)
        .map(|index| Rgb565Pixel(0x2000 + index as u16))
        .collect::<Vec<_>>();
    let mut buffers = NavigationTransitionBuffers::new(width, height);
    buffers.capture_source(&source).unwrap();
    buffers.capture_destination(&destination).unwrap();
    let progress_q16 = PROGRESS_MAX / 2;
    let travel_q16 = spring_ease_q16(progress_q16) as usize;

    render_settings_page_push(
        &mut buffers,
        NavigationTransitionRequest::settings_page_on_axis(
            NavigationTransitionDirection::Forward,
            SettingsPageTransitionAxis::Vertical,
        ),
        NavigationTransitionFrame {
            progress_q16,
            ..NavigationTransitionFrame::default()
        },
    )
    .unwrap();

    let source_rows = (height / SETTINGS_PAGE_SOURCE_TRAVEL_DIVISOR as usize) * travel_q16
        / PROGRESS_MAX as usize;
    let destination_y = height - height * travel_q16 / PROGRESS_MAX as usize;
    assert_eq!(
        &buffers.working()[..width],
        &source[source_rows * width..(source_rows + 1) * width]
    );
    assert_eq!(
        &buffers.working()[destination_y * width..(destination_y + 1) * width],
        &destination[..width]
    );
}

#[test]
fn settings_page_push_covers_every_pixel_without_a_clear_pass() {
    let width = 8;
    let height = 6;
    let source = vec![Rgb565Pixel(0x1111); width * height];
    let destination = vec![Rgb565Pixel(0x2222); width * height];
    let sentinel = Rgb565Pixel(0xffff);
    let mut buffers = NavigationTransitionBuffers::new(width, height);
    buffers.capture_source(&source).unwrap();
    buffers.capture_destination(&destination).unwrap();

    for axis in [
        SettingsPageTransitionAxis::Horizontal,
        SettingsPageTransitionAxis::Vertical,
        SettingsPageTransitionAxis::VerticalReversed,
    ] {
        for direction in [
            NavigationTransitionDirection::Forward,
            NavigationTransitionDirection::Reverse,
        ] {
            for progress_q16 in [1, 8_000, 16_000, 32_000, 48_000, 64_000] {
                let mut output = vec![sentinel; width * height];
                render_settings_page_transition_into(
                    &buffers,
                    NavigationTransitionRequest::settings_page_on_axis(direction, axis),
                    NavigationTransitionFrame {
                        progress_q16,
                        ..NavigationTransitionFrame::default()
                    },
                    &mut output,
                )
                .unwrap();
                assert!(output.iter().all(|pixel| *pixel != sentinel));
            }
        }
    }
}

#[test]
fn super_scaler_visual_windows_use_only_the_smooth_spring() {
    let source = include_str!("navigation.rs");
    let production = source
        .rsplit_once("\n#[cfg(test)]\nmod tests {")
        .expect("test module delimiter")
        .0;
    assert!(!production.contains("smoothstep_q16"));
    assert!(!production.contains("ease_out_cubic_q16"));
    assert!(!production.contains("with_overshoot"));
    assert!(!production.contains("recoil"));
    for (line_number, line) in production.lines().enumerate() {
        if line.contains("window_q16(") && !line.contains("fn window_q16(") {
            assert!(
                line.contains("spring_ease_q16(window_q16("),
                "raw-linear visual window at source line {}: {line}",
                line_number + 1
            );
        }
    }
}

#[test]
fn super_scaler_card_press_and_expansion_keep_exact_endpoints() {
    let source = geometry().source_card;
    let full = NavigationTransitionRect {
        x: 0,
        y: 0,
        width: 960,
        height: 540,
    };

    assert_eq!(super_scaler_card_rect(source, full, 0), source);
    let pressed = super_scaler_card_rect(source, full, 7_000);
    assert_eq!(pressed.x, source.x + 7);
    assert_eq!(pressed.y, source.y + 24);
    assert_eq!(pressed.width, source.width - 14);
    assert_eq!(pressed.height, source.height - 48);
    let launched = super_scaler_card_rect(source, full, 40_000);
    assert!(launched.x > 0);
    assert!(launched.right() > 900);
    assert!(launched.bottom() > 500);
    assert_eq!(super_scaler_card_rect(source, full, PROGRESS_MAX), full);
}

#[test]
fn super_scaler_keeps_exact_source_and_has_no_cover_reveal_surface_cut() {
    let width = 32;
    let height = 24;
    let source = (0..width * height)
        .map(|pixel| Rgb565Pixel((pixel as u16).wrapping_mul(17)))
        .collect::<Vec<_>>();
    let destination = (0..width * height)
        .map(|pixel| Rgb565Pixel((pixel as u16).wrapping_mul(29)))
        .collect::<Vec<_>>();
    let mut buffers = NavigationTransitionBuffers::new(width, height);
    buffers.capture_source(&source).unwrap();
    buffers.capture_destination(&destination).unwrap();
    let request = NavigationTransitionRequest::new(
        NavigationTransitionEdge::HomeToArcade,
        NavigationTransitionDirection::Forward,
        NavigationTransitionGeometry {
            source_card: NavigationTransitionRect {
                x: 2,
                y: 4,
                width: 8,
                height: 16,
            },
            source_label: NavigationTransitionRect {
                x: 3,
                y: 10,
                width: 6,
                height: 3,
            },
            destination_title: NavigationTransitionRect {
                x: 1,
                y: 1,
                width: 10,
                height: 3,
            },
            ..NavigationTransitionGeometry::default()
        },
    );
    let at_source = render_super_scaler_shell(
        &mut buffers,
        request,
        NavigationTransitionFrame {
            phase: NavigationTransitionPhase::Expand,
            owns_full_frame: true,
            ..NavigationTransitionFrame::default()
        },
    )
    .unwrap();
    assert_eq!(at_source.copied_pixels, source.len() as u64);
    assert_eq!(buffers.working(), source);

    let covered_stats = render_super_scaler_shell(
        &mut buffers,
        request,
        NavigationTransitionFrame {
            phase: NavigationTransitionPhase::Expand,
            progress_q16: SUPER_SCALER_COVER_PROGRESS,
            cover_progress_q16: PROGRESS_MAX,
            owns_full_frame: true,
            ..NavigationTransitionFrame::default()
        },
    )
    .unwrap();
    assert_eq!(covered_stats.copied_pixels, 0);
    assert!(covered_stats.filled_pixels >= source.len() as u64);
    let final_cover = buffers.working().to_vec();
    render_super_scaler_shell(
        &mut buffers,
        request,
        NavigationTransitionFrame {
            phase: NavigationTransitionPhase::Reveal,
            progress_q16: SUPER_SCALER_COVER_PROGRESS + 1,
            cover_progress_q16: PROGRESS_MAX,
            reveal_progress_q16: 1,
            owns_full_frame: true,
            ..NavigationTransitionFrame::default()
        },
    )
    .unwrap();
    assert_eq!(buffers.working(), final_cover);
}

#[test]
fn super_scaler_category_edges_keep_speed_bands_through_both_directions() {
    let width = 32;
    let height = 24;
    let shell = Rgb565Pixel(0x1028);
    let source = vec![Rgb565Pixel(0x1111); width * height];
    let destination = vec![Rgb565Pixel(0x2222); width * height];
    let mut buffers = NavigationTransitionBuffers::new(width, height);
    buffers.capture_source(&source).unwrap();
    buffers.capture_destination(&destination).unwrap();
    let geometry = NavigationTransitionGeometry {
        source_card: NavigationTransitionRect {
            x: 2,
            y: 4,
            width: 8,
            height: 16,
        },
        ..NavigationTransitionGeometry::default()
    };
    let request = NavigationTransitionRequest::new(
        NavigationTransitionEdge::HomeToConsoles,
        NavigationTransitionDirection::Forward,
        geometry,
    );

    render_super_scaler_shell(
        &mut buffers,
        request,
        NavigationTransitionFrame {
            phase: NavigationTransitionPhase::Expand,
            progress_q16: SUPER_SCALER_COVER_PROGRESS,
            cover_progress_q16: PROGRESS_MAX,
            owns_full_frame: true,
            ..NavigationTransitionFrame::default()
        },
    )
    .unwrap();
    let covered = buffers.working().to_vec();
    render_super_scaler_shell(
        &mut buffers,
        request,
        NavigationTransitionFrame {
            phase: NavigationTransitionPhase::Reveal,
            progress_q16: SUPER_SCALER_COVER_PROGRESS + 1,
            cover_progress_q16: PROGRESS_MAX,
            reveal_progress_q16: 1,
            owns_full_frame: true,
            ..NavigationTransitionFrame::default()
        },
    )
    .unwrap();
    assert_eq!(buffers.working(), covered);

    let mut concealed = source.clone();
    let mut stats = NavigationTransitionRenderStats::default();
    conceal_source_regions(
        &mut concealed,
        &source,
        width,
        height,
        60_000,
        NavigationTransitionRequest {
            direction: NavigationTransitionDirection::Reverse,
            ..request
        },
        shell,
        &mut stats,
    );
    assert_eq!(
        concealed[21 * width],
        super_scaler_shell_row_color(21, height, shell)
    );
    let mut expected_reverse_cover = vec![shell; width * height];
    fill_super_scaler_covered_surface(
        &mut expected_reverse_cover,
        width,
        height,
        NavigationTransitionRect {
            x: 0,
            y: 0,
            width: width as u16,
            height: height as u16,
        },
        shell,
        &mut stats,
    );
    assert_eq!(concealed, expected_reverse_cover);
}

#[test]
fn system_background_opens_as_one_horizon_instead_of_scanline_moire() {
    let width = 12;
    let height = 16;
    let shell = Rgb565Pixel(0x1111);
    let mut destination = vec![Rgb565Pixel(0x2222); width * height];
    for row in 0..height {
        destination[row * width] = Rgb565Pixel(0xf800);
        destination[row * width + width - 1] = Rgb565Pixel(0xf800);
    }
    let mut working = vec![shell; width * height];
    let mut stats = NavigationTransitionRenderStats::default();

    compose_system_background_horizon(
        &mut working,
        &destination,
        width,
        height,
        PROGRESS_MAX / 2,
        4,
        shell,
        &mut stats,
    );

    let destination_rows = (0..height)
        .filter(|row| working[row * width] == Rgb565Pixel(0x2222))
        .collect::<Vec<_>>();
    assert!(!destination_rows.is_empty());
    assert!(
        destination_rows
            .windows(2)
            .all(|rows| rows[1] == rows[0] + 1)
    );
    assert!(destination_rows.contains(&4));
    assert!(!working.contains(&Rgb565Pixel(0xf800)));
}

#[test]
fn scaled_card_excludes_the_duplicate_label_surface() {
    let width = 8;
    let height = 8;
    let mut source = vec![Rgb565Pixel(0x1111); width * height];
    let card = NavigationTransitionRect {
        x: 1,
        y: 1,
        width: 6,
        height: 6,
    };
    let label = NavigationTransitionRect {
        x: 3,
        y: 3,
        width: 2,
        height: 2,
    };
    for y in label.y as usize..label.bottom() as usize {
        for x in label.x as usize..label.right() as usize {
            source[y * width + x] = Rgb565Pixel(0xffff);
        }
    }
    let mut destination = vec![Rgb565Pixel(0x2222); width * height];
    let mut scale_source_x = vec![0; width];
    let mut scale_source_y = vec![0; height];
    let mut scale_excluded_x = vec![false; width];
    let mut scale_dither_x = vec![false; width * 4];
    let mut stats = NavigationTransitionRenderStats::default();
    blit_scaled_card_565(
        &mut destination,
        &source,
        width,
        height,
        card,
        card,
        label,
        PROGRESS_MAX,
        &mut scale_source_x,
        &mut scale_source_y,
        &mut scale_excluded_x,
        &mut scale_dither_x,
        &mut stats,
    );

    assert_eq!(destination[3 * width + 3], Rgb565Pixel(0x2222));
    assert_eq!(destination[width + 1], Rgb565Pixel(0x1111));
}

#[test]
fn super_scaler_echoes_remain_visible_above_the_expanding_card() {
    let width = 64;
    let height = 48;
    let source = vec![Rgb565Pixel(0x1111); width * height];
    let mut buffers = NavigationTransitionBuffers::new(width, height);
    buffers.capture_source(&source).unwrap();
    let request = NavigationTransitionRequest::new(
        NavigationTransitionEdge::HomeToArcade,
        NavigationTransitionDirection::Forward,
        NavigationTransitionGeometry {
            source_card: NavigationTransitionRect {
                x: 10,
                y: 8,
                width: 18,
                height: 32,
            },
            ..NavigationTransitionGeometry::default()
        },
    );

    render_super_scaler_shell(
        &mut buffers,
        request,
        NavigationTransitionFrame {
            phase: NavigationTransitionPhase::Expand,
            progress_q16: 20_000,
            cover_progress_q16: 35_000,
            owns_full_frame: true,
            ..NavigationTransitionFrame::default()
        },
    )
    .unwrap();

    assert!(buffers.working().contains(&Rgb565Pixel(0x79b8)));
    assert!(buffers.working().contains(&Rgb565Pixel(0x40ed)));
    assert!(buffers.working().contains(&Rgb565Pixel(0x28aa)));
}

#[test]
fn zero_opacity_detail_draw_does_not_erase_the_expanding_shell() {
    let width = 12;
    let height = 8;
    let mut snapshot = vec![Rgb565Pixel(0x0000); width * height];
    snapshot[3 * width + 5] = Rgb565Pixel(0xffff);
    let mut working = vec![Rgb565Pixel(0x1028); width * height];
    let original = working.clone();
    let mut stats = NavigationTransitionRenderStats::default();

    draw_detail_pixels_with_opacity(
        &mut working,
        &snapshot,
        width,
        height,
        NavigationTransitionRect {
            x: 3,
            y: 2,
            width: 5,
            height: 3,
        },
        0,
        &mut stats,
    );

    assert_eq!(working, original);
    assert_eq!(stats, NavigationTransitionRenderStats::default());
}

#[test]
fn forward_hero_title_docks_to_left_aligned_destination() {
    let from = NavigationTransitionRect {
        x: 20,
        y: 100,
        width: 180,
        height: 16,
    };
    let centered_content = NavigationTransitionRect {
        x: 80,
        y: 103,
        width: 60,
        height: 10,
    };
    let destination = NavigationTransitionRect {
        x: 16,
        y: 16,
        width: 160,
        height: 24,
    };

    let target = label_target_rect(centered_content, from, destination, false);

    assert_eq!(target.x, destination.x);
    assert_eq!(target.y, 20);
    assert_eq!(target.height, 15);
}

#[test]
fn final_region_reveal_is_the_exact_destination() {
    let width = 16;
    let height = 12;
    let mut working = vec![Rgb565Pixel(0); width * height];
    let destination = (0..width * height)
        .map(|pixel| Rgb565Pixel(pixel as u16))
        .collect::<Vec<_>>();
    let mut stats = NavigationTransitionRenderStats::default();

    reveal_destination_regions(
        &mut working,
        &destination,
        width,
        height,
        62_000,
        request(),
        &mut stats,
    );

    assert_eq!(working, destination);
}

#[test]
fn system_reveal_orders_title_rows_frame_and_preview_content() {
    let width = 100;
    let height = 100;
    let mut destination = vec![Rgb565Pixel(0); width * height];
    for y in 2..10 {
        destination[y * width + 2..y * width + 32].fill(Rgb565Pixel(0x1234));
    }
    for y in 18..24 {
        destination[y * width + 8..y * width + 55].fill(Rgb565Pixel(0x4567));
    }
    for y in 46..52 {
        destination[y * width + 8..y * width + 55].fill(Rgb565Pixel(0x1234));
    }
    for y in 70..76 {
        destination[y * width + 8..y * width + 55].fill(Rgb565Pixel(0x1234));
    }
    for y in 18..94 {
        destination[y * width + 58..y * width + 96].fill(Rgb565Pixel(0x1234));
    }
    let mut stats = NavigationTransitionRenderStats::default();

    let mut title_only = vec![Rgb565Pixel(0); width * height];
    reveal_destination_regions(
        &mut title_only,
        &destination,
        width,
        height,
        8_000,
        system_request(NavigationTransitionDirection::Forward),
        &mut stats,
    );
    assert_eq!(title_only[5 * width + 5], Rgb565Pixel(0x1234));
    assert_eq!(title_only[20 * width + 20], Rgb565Pixel(0));
    assert_eq!(title_only[46 * width + 20], Rgb565Pixel(0));

    let mut selected_row = vec![Rgb565Pixel(0); width * height];
    reveal_destination_regions(
        &mut selected_row,
        &destination,
        width,
        height,
        22_000,
        system_request(NavigationTransitionDirection::Forward),
        &mut stats,
    );
    assert_eq!(selected_row[48 * width + 20], Rgb565Pixel(0x1234));
    assert_eq!(selected_row[70 * width + 20], Rgb565Pixel(0));

    let mut framed = vec![Rgb565Pixel(0); width * height];
    reveal_destination_regions(
        &mut framed,
        &destination,
        width,
        height,
        46_000,
        system_request(NavigationTransitionDirection::Forward),
        &mut stats,
    );
    assert_eq!(framed[18 * width + 77], Rgb565Pixel(0x79b8));
    assert_eq!(framed[50 * width + 77], Rgb565Pixel(0x1234));

    let mut content = vec![Rgb565Pixel(0); width * height];
    reveal_destination_regions(
        &mut content,
        &destination,
        width,
        height,
        60_000,
        system_request(NavigationTransitionDirection::Forward),
        &mut stats,
    );
    assert_eq!(content[50 * width + 77], Rgb565Pixel(0x1234));
}

#[test]
fn preview_rails_pulse_without_popping_at_forward_or_reverse_endpoints() {
    assert_eq!(preview_rail_envelope(34_000), 0);
    assert!(preview_rail_envelope(40_000) > 0);
    assert_eq!(preview_rail_envelope(42_000), PROGRESS_MAX);
    assert_eq!(preview_rail_envelope(44_000), PROGRESS_MAX);
    assert_eq!(preview_rail_envelope(48_000), PROGRESS_MAX);
    assert_eq!(preview_rail_envelope(58_000), 0);
    assert_eq!(preview_rail_envelope(61_999), 0);
    assert_eq!(
        preview_rail_envelope(reverse_destination_reveal_progress(0)),
        0
    );
    assert_eq!(
        preview_rail_envelope(reverse_destination_reveal_progress(14_000)),
        PROGRESS_MAX
    );
    assert_eq!(
        preview_rail_envelope(reverse_destination_reveal_progress(28_000)),
        0
    );
}

#[test]
fn preview_aperture_opens_from_a_horizontal_scanline() {
    let preview = NavigationTransitionRect {
        x: 100,
        y: 80,
        width: 320,
        height: 240,
    };
    let slit = preview_aperture_rect(preview, 8_000).unwrap();
    assert!(slit.width > 64);
    assert!(slit.height <= 2);
    assert_eq!(preview_aperture_rect(preview, PROGRESS_MAX), Some(preview));
}

#[test]
fn system_reverse_reconstructs_exact_forward_reveal_endpoints() {
    let width = 100;
    let height = 100;
    let source = vec![Rgb565Pixel(0x1234); width * height];
    let shell = Rgb565Pixel(0x1028);
    let mut working = source.clone();
    let mut stats = NavigationTransitionRenderStats::default();

    conceal_source_regions_inverse(
        &mut working,
        &source,
        width,
        height,
        0,
        system_request(NavigationTransitionDirection::Reverse),
        shell,
        &mut stats,
    );

    assert_eq!(working, source);

    conceal_source_regions_inverse(
        &mut working,
        &source,
        width,
        height,
        PROGRESS_MAX,
        system_request(NavigationTransitionDirection::Reverse),
        shell,
        &mut stats,
    );
    let mut expected = vec![shell; width * height];
    fill_super_scaler_covered_surface(
        &mut expected,
        width,
        height,
        NavigationTransitionRect {
            x: 0,
            y: 0,
            width: width as u16,
            height: height as u16,
        },
        shell,
        &mut stats,
    );
    assert_eq!(working, expected);
}

#[test]
fn shifted_row_copy_ignores_equal_undersized_buffers() {
    let source = vec![Rgb565Pixel(0xaaaa); 8];
    let mut working = vec![Rgb565Pixel(0); 8];
    let mut stats = NavigationTransitionRenderStats::default();

    copy_rect_shifted_x(
        &mut working,
        &source,
        8,
        8,
        NavigationTransitionRect {
            x: 0,
            y: 0,
            width: 8,
            height: 8,
        },
        PROGRESS_MAX,
        -28,
        &mut stats,
    );

    assert!(working.iter().all(|pixel| *pixel == Rgb565Pixel(0)));
}

#[test]
fn label_crossfade_uses_one_moving_surface_without_dither_holes() {
    let width = 16;
    let height = 8;
    let mut source = vec![Rgb565Pixel(0); width * height];
    let mut destination = vec![Rgb565Pixel(0); width * height];
    let source_rect = NavigationTransitionRect {
        x: 1,
        y: 1,
        width: 4,
        height: 4,
    };
    let destination_rect = NavigationTransitionRect {
        x: 10,
        y: 2,
        width: 4,
        height: 4,
    };
    let target = NavigationTransitionRect {
        x: 6,
        y: 2,
        width: 4,
        height: 4,
    };
    for y in source_rect.y as usize..source_rect.bottom() as usize {
        source[y * width + source_rect.x as usize..y * width + source_rect.right() as usize]
            .fill(Rgb565Pixel(0x1111));
    }
    for y in destination_rect.y as usize..destination_rect.bottom() as usize {
        destination[y * width + destination_rect.x as usize
            ..y * width + destination_rect.right() as usize]
            .fill(Rgb565Pixel(0xeeee));
    }
    let mut working = vec![Rgb565Pixel(0); width * height];
    let mut stats = NavigationTransitionRenderStats::default();

    blit_crossfaded_masks_565(
        &mut working,
        &source,
        &destination,
        width,
        height,
        source_rect,
        destination_rect,
        target,
        Rgb565Pixel(0),
        Rgb565Pixel(0),
        PROGRESS_MAX / 2,
        &mut stats,
    );

    for y in target.y as usize..target.bottom() as usize {
        for x in target.x as usize..target.right() as usize {
            assert_ne!(working[y * width + x], Rgb565Pixel(0));
        }
    }
    assert_eq!(working[2 * width + 10], Rgb565Pixel(0));
}

#[test]
fn label_crossfade_deterministically_erodes_disjoint_glyph_masks() {
    let width = 12;
    let height = 6;
    let rect = NavigationTransitionRect {
        x: 2,
        y: 1,
        width: 8,
        height: 4,
    };
    let mut source = vec![Rgb565Pixel(0); width * height];
    let mut destination = vec![Rgb565Pixel(0); width * height];
    for y in rect.y as usize..rect.bottom() as usize {
        source[y * width + 2..y * width + 6].fill(Rgb565Pixel(0x1111));
        destination[y * width + 6..y * width + 10].fill(Rgb565Pixel(0xeeee));
    }
    let mut first = vec![Rgb565Pixel(0); width * height];
    let mut second = first.clone();
    let mut stats = NavigationTransitionRenderStats::default();

    for working in [&mut first, &mut second] {
        blit_crossfaded_masks_565(
            working,
            &source,
            &destination,
            width,
            height,
            rect,
            rect,
            rect,
            Rgb565Pixel(0),
            Rgb565Pixel(0),
            PROGRESS_MAX / 2,
            &mut stats,
        );
    }

    assert_eq!(first, second);
    assert!(first.contains(&Rgb565Pixel(0x1111)));
    assert!(first.contains(&Rgb565Pixel(0xeeee)));
    assert!(first.contains(&Rgb565Pixel(0)));
}

#[test]
fn reverse_row_translation_preserves_source_alignment_and_clipping() {
    let width = 20;
    let height = 8;
    let rect = NavigationTransitionRect {
        x: 8,
        y: 6,
        width: 8,
        height: 2,
    };
    let shell = Rgb565Pixel(0x2222);
    let mut source = vec![Rgb565Pixel(0); width * height];
    for y in rect.y as usize..rect.bottom() as usize {
        for x in rect.x as usize..rect.right() as usize {
            source[y * width + x] = Rgb565Pixel(x as u16);
        }
    }
    let mut stats = NavigationTransitionRenderStats::default();

    let mut at_start = source.clone();
    slide_rect_out_left(
        &mut at_start,
        &source,
        width,
        height,
        rect,
        0,
        shell,
        &mut stats,
    );
    assert_eq!(
        &at_start[6 * width + 8..6 * width + 16],
        &source[6 * width + 8..6 * width + 16]
    );

    let mut halfway = vec![Rgb565Pixel(0); width * height];
    slide_rect_out_left(
        &mut halfway,
        &source,
        width,
        height,
        rect,
        PROGRESS_MAX / 2 + 1,
        shell,
        &mut stats,
    );
    assert_eq!(halfway[6 * width], Rgb565Pixel(8));
    assert_eq!(halfway[6 * width + 7], Rgb565Pixel(15));
    assert_eq!(halfway[7 * width], Rgb565Pixel(8));

    let mut nearly_gone = source.clone();
    slide_rect_out_left(
        &mut nearly_gone,
        &source,
        width,
        height,
        rect,
        PROGRESS_MAX - 1,
        shell,
        &mut stats,
    );
    assert_eq!(nearly_gone[6 * width], Rgb565Pixel(15));
    assert_eq!(nearly_gone[6 * width + 1], Rgb565Pixel(0));

    let mut at_end = vec![Rgb565Pixel(0); width * height];
    slide_rect_out_left(
        &mut at_end,
        &source,
        width,
        height,
        rect,
        PROGRESS_MAX,
        shell,
        &mut stats,
    );
    assert!(
        at_end[6 * width + 8..6 * width + 16]
            .iter()
            .all(|pixel| *pixel == shell)
    );
}

#[test]
fn forward_row_translation_enters_from_the_first_screen_pixel() {
    let width = 20;
    let height = 8;
    let rect = NavigationTransitionRect {
        x: 8,
        y: 6,
        width: 8,
        height: 2,
    };
    let mut source = vec![Rgb565Pixel(0); width * height];
    for y in rect.y as usize..rect.bottom() as usize {
        for x in rect.x as usize..rect.right() as usize {
            source[y * width + x] = Rgb565Pixel(x as u16);
        }
    }
    let mut working = vec![Rgb565Pixel(0); width * height];
    let mut stats = NavigationTransitionRenderStats::default();

    copy_rect_shifted_x(
        &mut working,
        &source,
        width,
        height,
        rect,
        1,
        -(rect.right() as isize),
        &mut stats,
    );

    assert_eq!(working[6 * width], Rgb565Pixel(15));
    assert_eq!(working[6 * width + 1], Rgb565Pixel(0));
}

#[test]
fn selected_row_enters_monotonically_and_settles_exactly() {
    let width = 32;
    let height = 8;
    let rect = NavigationTransitionRect {
        x: 8,
        y: 3,
        width: 8,
        height: 2,
    };
    let mut source = vec![Rgb565Pixel(0); width * height];
    source[3 * width + 8..3 * width + 16].fill(Rgb565Pixel(0xaaaa));
    let mut stats = NavigationTransitionRenderStats::default();
    let initial_offset = -(rect.right() as isize + SYSTEM_ROW_OFFSCREEN_MARGIN);
    let mut previous_right = 0;
    let mut settled = Vec::new();
    for phase in [0, 8_000, 16_000, 24_000, 32_000, 48_000, PROGRESS_MAX] {
        let mut frame = vec![Rgb565Pixel(0); width * height];
        copy_rect_shifted_x(
            &mut frame,
            &source,
            width,
            height,
            rect,
            spring_ease_q16(phase),
            initial_offset,
            &mut stats,
        );
        let right = frame[3 * width..4 * width]
            .iter()
            .rposition(|pixel| *pixel == Rgb565Pixel(0xaaaa))
            .map_or(0, |x| x + 1);
        assert!(
            right >= previous_right,
            "row moved backwards at phase {phase}"
        );
        assert!(
            right <= rect.right() as usize,
            "row overshot its destination"
        );
        previous_right = right;
        settled = frame;
    }
    assert_eq!(
        &settled[3 * width + 8..3 * width + 16],
        &source[3 * width + 8..3 * width + 16]
    );
}

#[test]
fn reverse_selected_row_exits_monotonically_without_recoil() {
    let width = 32;
    let height = 8;
    let shell = Rgb565Pixel(0x2222);
    let rect = NavigationTransitionRect {
        x: 24,
        y: 3,
        width: 8,
        height: 2,
    };
    let mut source = vec![Rgb565Pixel(0); width * height];
    for x in rect.x as usize..rect.right() as usize {
        source[3 * width + x] = Rgb565Pixel(x as u16);
    }
    let mut stats = NavigationTransitionRenderStats::default();
    let mut previous_left = rect.x as usize;
    let mut gone = source.clone();
    for phase in [0, 8_000, 16_000, 24_000, 32_000, 48_000, PROGRESS_MAX] {
        let mut frame = source.clone();
        slide_rect_out_left(
            &mut frame,
            &source,
            width,
            height,
            rect,
            spring_ease_q16(phase),
            shell,
            &mut stats,
        );
        let left = frame[3 * width..4 * width]
            .iter()
            .position(|pixel| *pixel != shell && *pixel != Rgb565Pixel(0))
            .unwrap_or(0);
        assert!(left <= previous_left, "row recoiled at phase {phase}");
        previous_left = left;
        gone = frame;
    }
    assert!(
        gone[3 * width + 24..3 * width + 32]
            .iter()
            .all(|pixel| *pixel == shell)
    );
}

#[test]
fn reverse_preview_aperture_keeps_identity_and_closes_exactly() {
    let width = 20;
    let height = 12;
    let shell = Rgb565Pixel(0x2222);
    let preview = NavigationTransitionRect {
        x: 4,
        y: 3,
        width: 12,
        height: 6,
    };
    let source = vec![Rgb565Pixel(0xaaaa); width * height];
    let mut stats = NavigationTransitionRenderStats::default();
    let mut unchanged = source.clone();
    close_preview_aperture(
        &mut unchanged,
        &source,
        width,
        height,
        preview,
        0,
        shell,
        &mut stats,
    );
    assert_eq!(unchanged, source);

    let mut closed = source.clone();
    close_preview_aperture(
        &mut closed,
        &source,
        width,
        height,
        preview,
        PROGRESS_MAX,
        shell,
        &mut stats,
    );
    for y in preview.y as usize..preview.bottom() as usize {
        assert!(
            closed[y * width + preview.x as usize..y * width + preview.right() as usize]
                .iter()
                .all(|pixel| *pixel == shell)
        );
    }
}

#[test]
fn wipe_helpers_clip_saturated_rectangles_without_panicking() {
    let width = 8;
    let height = 6;
    let source = vec![Rgb565Pixel(0xaaaa); width * height];
    let mut working = vec![Rgb565Pixel(0); width * height];
    let mut stats = NavigationTransitionRenderStats::default();
    let saturated = NavigationTransitionRect {
        x: 6,
        y: 4,
        width: u16::MAX,
        height: u16::MAX,
    };

    copy_rect_horizontal_wipe(
        &mut working,
        &source,
        width,
        height,
        saturated,
        PROGRESS_MAX,
        usize::MAX,
        &mut stats,
    );
    copy_rect_vertical_wipe(
        &mut working,
        &source,
        width,
        height,
        saturated,
        PROGRESS_MAX,
        false,
        &mut stats,
    );

    assert_eq!(working[4 * width + 6], Rgb565Pixel(0xaaaa));
    assert_eq!(working[5 * width + 7], Rgb565Pixel(0xaaaa));
}
