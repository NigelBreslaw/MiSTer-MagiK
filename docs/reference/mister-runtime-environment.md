# MiSTer runtime environment reference

<!-- Generated from apps/mister/config/runtime-environment.toml. Do not edit. -->

Registry format: `mister-magik-runtime-environment-v1`. Baseline: 396 literal occurrences, 274 owned names, 10 external/build-time names.

| Name | Classification | Shape | Default behavior | Visibility | Owner |
|---|---|---|---|---|---|
| `MISTER_7ZA` | external | string, enum, or boolean token | site-defined fallback; unchanged in P0 | external compatibility | `crates/catalog/src/media_metadata.rs` |
| `MISTER_7ZA_TIMEOUT_SECS` | external | bounded integer | site-defined fallback; unchanged in P0 | external compatibility | `crates/catalog/src/media_metadata.rs` |
| `MISTER_AMIGA_PREVIEW_ARCHIVE` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_ANIMATION_CLOCK` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/visual_platform.rs` |
| `MISTER_ARCADE_BOOTSTRAP_INDEX` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/pmu_profile.rs` |
| `MISTER_ARCADE_ENTRY_RUN_ID` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_ARCADE_ENTRY_TRACE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_ARCADE_ROOT` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/effect_loop_support.rs` |
| `MISTER_ARCADE_SCROLL_SPEED_DIV` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/launcher.rs` |
| `MISTER_ARCADE_SELECTED_INDEX` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/ui_frame_target.rs` |
| `MISTER_ARCADE_SELECTION_INVERT` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/arcade_list_renderer.rs` |
| `MISTER_BACKGROUND_AFFINITY` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/runtime_thread.rs` |
| `MISTER_BOOT_ANALYTICS` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `mister/platform/runtime/src/boot_analytics.rs` |
| `MISTER_BOOT_BLACK_SETTLE_FRAMES` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/ui_boot.rs` |
| `MISTER_BOOT_FRAME_PROFILE_FILE` | diagnostic | path or path list | site-defined fallback; unchanged in P0 | developer diagnostic | `mister/platform/runtime/src/boot_analytics.rs` |
| `MISTER_BOOT_FRAME_PROFILE_FRAMES` | diagnostic | bounded integer | site-defined fallback; unchanged in P0 | developer diagnostic | `mister/platform/runtime/src/boot_analytics.rs` |
| `MISTER_CAMERA_EFFECTS` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/camera_effects_loop.rs` |
| `MISTER_CAMERA_EFFECTS_AUTO` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/camera_effects_loop.rs` |
| `MISTER_CAMERA_EFFECTS_CACHE_CAP` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/camera_effects_loop.rs` |
| `MISTER_CAMERA_EFFECTS_HUD` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/camera_effects_loop.rs` |
| `MISTER_CAMERA_EFFECTS_SEGMENT_SECS` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/camera_effects_loop.rs` |
| `MISTER_CAMERA_EFFECTS_TRACE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/experiments/effects/camera_effects_loop.rs` |
| `MISTER_CATALOG_BACKGROUND_DELAY_MS` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_CATALOG_BUILDER_LOCK` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/pmu_profile.rs` |
| `MISTER_CATALOG_CONTENTION_QUIET_PREVIEWS` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_CATALOG_DIAGNOSTICS_DIR` | diagnostic | path or path list | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/catalog_progress_report.rs` |
| `MISTER_CATALOG_DURABLE_RESUME` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/library_indexer.rs` |
| `MISTER_CATALOG_PROTOCOL_STDOUT` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/bin/catalog_builder.rs` |
| `MISTER_CATALOG_READY_SNAPSHOT` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/pmu_profile.rs` |
| `MISTER_CATALOG_REFRESH` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/ui_frame_target.rs` |
| `MISTER_CATALOG_TRACE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `crates/catalog/src/catalog_checkpoint.rs` |
| `MISTER_CLOUD` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `crates/particles/src/intro.rs` |
| `MISTER_CRASH_BACKTRACE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/crash_report.rs` |
| `MISTER_CRT_PROBE_PATTERN` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/crt_trial_loop.rs` |
| `MISTER_CRT_TRIAL_CONTENT_BOUNDS` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/crt_trial_loop.rs` |
| `MISTER_DIRTY_RECT_BROAD_PCT` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `mister/platform/runtime/src/framebuffer/target.rs` |
| `MISTER_DISPLAY_OWNER_LOCK` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `mister/platform/runtime/src/framebuffer/ownership.rs` |
| `MISTER_EFFECT_BENCH_LABEL` | benchmark | string, enum, or boolean token | site-defined fallback; unchanged in P0 | benchmark only | `apps/mister/src/ui_effect_bench.rs` |
| `MISTER_FB_DIAGNOSTIC_RECT` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `mister/platform/runtime/src/fpga.rs` |
| `MISTER_FB_MAP_BANDWIDTH_FRAMES` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/main.rs` |
| `MISTER_FB_MAP_BANDWIDTH_H` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/main.rs` |
| `MISTER_FB_MAP_BANDWIDTH_W` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/main.rs` |
| `MISTER_FB_MAP_REPORT_H` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/main.rs` |
| `MISTER_FB_MAP_REPORT_W` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/main.rs` |
| `MISTER_FB_PRESENT_DELAY_US` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/visual_platform.rs` |
| `MISTER_FB_RIGHT_GUARD_COLS` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_present/latch.rs` |
| `MISTER_FB_ROUTE_REASSERT_FRAMES` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `mister/platform/runtime/src/framebuffer/ownership.rs` |
| `MISTER_FPGA_LATCH_PATTERN_FRAMES` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/main.rs` |
| `MISTER_FPGA_LATCH_PATTERN_PERIOD_US` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/main.rs` |
| `MISTER_FRAMEBUFFER_STREAM_SCALE` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `mister/platform/runtime/src/framebuffer/stream.rs` |
| `MISTER_FRAME_ORDER` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/video_loop.rs` |
| `MISTER_FS_FAULT_ACTION` | fault | string, enum, or boolean token | site-defined fallback; unchanged in P0 | destructive test only | `crates/catalog/src/fs_fault.rs` |
| `MISTER_FS_FAULT_DELAY_MS` | fault | bounded integer | site-defined fallback; unchanged in P0 | destructive test only | `crates/catalog/src/fs_fault.rs` |
| `MISTER_FS_FAULT_POINT` | fault | string, enum, or boolean token | site-defined fallback; unchanged in P0 | destructive test only | `crates/catalog/src/fs_fault.rs` |
| `MISTER_FS_FAULT_SESSION` | fault | string, enum, or boolean token | site-defined fallback; unchanged in P0 | destructive test only | `crates/catalog/src/fs_fault.rs` |
| `MISTER_GLYPH_ALPHA_THRESHOLD` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `mister/platform/runtime/src/framebuffer/mapped.rs` |
| `MISTER_GROUPS` | external | string, enum, or boolean token | site-defined fallback; unchanged in P0 | external compatibility | `crates/particles/src/intro.rs` |
| `MISTER_GUI_FRAME_PROFILE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launcher_gui_profile.rs` |
| `MISTER_GUI_FRAME_PROFILE_COMPLETE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launcher_gui_profile.rs` |
| `MISTER_GUI_FRAME_PROFILE_PMU` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launcher_gui_profile.rs` |
| `MISTER_HBMAME_SQLITE` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/catalog_config.rs` |
| `MISTER_HOME_SELECTED_INDEX` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_HUMAN_TURBO_IDLE_FRAMES` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_bench.rs` |
| `MISTER_HUMAN_TURBO_NORMAL_FRAMES` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_bench.rs` |
| `MISTER_HUMAN_TURBO_PAUSE_FRAMES` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_bench.rs` |
| `MISTER_INI_PATH` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_display.rs` |
| `MISTER_INPUT_INTEGRITY_STALL_MS` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_INPUT_INTEGRITY_TRACE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_INPUT_LATENCY_LAB_ARM` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launcher_input_latency_lab.rs` |
| `MISTER_INPUT_LATENCY_LAB_READER_POLICY` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/input_hub.rs` |
| `MISTER_INPUT_LATENCY_LAB_READER_SCHEDSTAT` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/input_hub.rs` |
| `MISTER_INPUT_LATENCY_LAB_SESSION` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/input_hub.rs` |
| `MISTER_LATCH_V5_QUALIFICATION` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/latch_v5_qualification.rs` |
| `MISTER_LAUNCHER_AUTO_LAUNCH_SELECTED` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_LAUNCHER_BENCH_AFTER_INPUT_SCRIPT` | benchmark | string, enum, or boolean token | site-defined fallback; unchanged in P0 | benchmark only | `apps/mister/src/ui_runner/launcher_bench.rs` |
| `MISTER_LAUNCHER_BENCH_SCENARIO` | benchmark | string, enum, or boolean token | site-defined fallback; unchanged in P0 | benchmark only | `apps/mister/src/ui_runner/launcher_bench.rs` |
| `MISTER_LAUNCHER_DIRTY_OPT` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/ui_frame_target.rs` |
| `MISTER_LAUNCHER_INPUT_SCRIPT` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_LAUNCHER_INPUT_SCRIPT_WAIT_FRAMES` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_LAUNCHER_LOCK_SCREEN` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_bench.rs` |
| `MISTER_LAUNCHER_RESPONSE_COMPLETE` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_LAUNCHER_RESPONSE_EXECUTION_TRACE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_LAUNCHER_RESPONSE_EXPECTED_CONFIRMED` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_LAUNCHER_RESPONSE_EXPECTED_FEEDBACK_HIDDEN` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_LAUNCHER_RESPONSE_FRAME_COMPLETE` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_LAUNCHER_RESPONSE_PMU` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_LAUNCHER_RESPONSE_PMU_COMPLETE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_LAUNCHER_RESPONSE_RUN_ID` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_LAUNCHER_RESPONSE_TRACE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_LAUNCHER_START_MENU` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_bench.rs` |
| `MISTER_LAUNCHER_START_SCREEN` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_bench.rs` |
| `MISTER_LAUNCHER_START_SYSTEM` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_bench.rs` |
| `MISTER_LAUNCH_HANDOFF_DELAY_MS` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launch_handoff_session.rs` |
| `MISTER_LAUNCH_HANDOFF_ITERATIONS` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launch_handoff_session.rs` |
| `MISTER_LAUNCH_HANDOFF_LABEL` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launch_handoff_session.rs` |
| `MISTER_LAUNCH_HANDOFF_MODE` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launch_handoff_session.rs` |
| `MISTER_LAUNCH_HANDOFF_TRACE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launch_handoff_session.rs` |
| `MISTER_LAUNCH_PREP_AMIGAVISION_LIMIT` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/launch_preparation.rs` |
| `MISTER_LAUNCH_PREP_ITERATIONS` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/launch_preparation.rs` |
| `MISTER_LAUNCH_PREP_LABEL` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/launch_preparation.rs` |
| `MISTER_LAUNCH_PREP_REFS` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/launch_preparation.rs` |
| `MISTER_LAUNCH_PREP_VIRTUAL_LIMIT` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/launch_preparation.rs` |
| `MISTER_LAUNCH_PREP_VIRTUAL_SYSTEMS` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/launch_preparation.rs` |
| `MISTER_LAUNCH_RETURN_PMU_HANDOFF_OUT` | diagnostic | path or path list | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launch_handoff_session.rs` |
| `MISTER_LIBRARY_BENCH_FORCE_REBUILD` | benchmark | string, enum, or boolean token | site-defined fallback; unchanged in P0 | benchmark only | `crates/catalog/src/library_cli.rs` |
| `MISTER_LIBRARY_BENCH_ITERATIONS` | benchmark | bounded integer | site-defined fallback; unchanged in P0 | benchmark only | `crates/catalog/src/library_cli.rs` |
| `MISTER_LIBRARY_BENCH_LABEL` | benchmark | string, enum, or boolean token | site-defined fallback; unchanged in P0 | benchmark only | `crates/catalog/src/library_cli.rs` |
| `MISTER_LIBRARY_BENCH_PRECOUNT` | benchmark | string, enum, or boolean token | site-defined fallback; unchanged in P0 | benchmark only | `crates/catalog/src/library_cli.rs` |
| `MISTER_LIBRARY_BENCH_SQLITE` | benchmark | path or path list | site-defined fallback; unchanged in P0 | benchmark only | `crates/catalog/src/device_layout.rs` |
| `MISTER_LIBRARY_NAMESPACE_BACKEND` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/namespace_walk.rs` |
| `MISTER_LIBRARY_PATH_MAP` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/catalog_config.rs` |
| `MISTER_LIBRARY_REFRESH_LOCK` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/main.rs` |
| `MISTER_LIBRARY_ROOTS` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/pmu_profile.rs` |
| `MISTER_LIBRARY_SOFTWARE_HASH` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/software_identity.rs` |
| `MISTER_LIBRARY_SQLITE` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/pmu_profile.rs` |
| `MISTER_LIBRARY_SQLITE_BUILD_DIR` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/pmu_profile.rs` |
| `MISTER_LOW_MEMORY_AVAILABLE_KIB` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/memory_pressure.rs` |
| `MISTER_MAGIK_BUILD_NUMBER` | build-time | string, enum, or boolean token | site-defined fallback; unchanged in P0 | build pipeline | `apps/mister/src/build_identity.rs` |
| `MISTER_MAGIK_BUILD_TIME` | build-time | string, enum, or boolean token | site-defined fallback; unchanged in P0 | build pipeline | `apps/mister/src/build_identity.rs` |
| `MISTER_MAGIK_CRT_TRIAL` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `mister/platform/runtime/src/fpga.rs` |
| `MISTER_MAGIK_DEV_LATCH_POST_SKIP_WORD_INDEX` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_present/latch.rs` |
| `MISTER_MAGIK_DEV_LATCH_STATUS_TIMEOUT_AT` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_present/latch.rs` |
| `MISTER_MAGIK_DISPLAY_CONFIRM_UI` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_MAGIK_DOWNLOADER_INI` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/update_checker.rs` |
| `MISTER_MAGIK_HOT_JOURNAL_DB` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/sqlite_catalog.rs` |
| `MISTER_MAGIK_INPUT_PROXY` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/input_hub.rs` |
| `MISTER_MAGIK_INPUT_PROXY_PROTOCOL` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/input_hub.rs` |
| `MISTER_MAGIK_MAIN_GENERATION` | production | bounded integer | missing or invalid disables supervised readiness acknowledgement | internal runtime | `apps/mister/src/process_config.rs` |
| `MISTER_MAGIK_MAIN_PID` | production | bounded integer | missing or invalid disables supervised readiness acknowledgement | internal runtime | `apps/mister/src/process_config.rs` |
| `MISTER_MAGIK_OWNER_EPOCH` | production | bounded integer | missing or invalid disables supervised readiness acknowledgement | internal runtime | `apps/mister/src/process_config.rs` |
| `MISTER_MAGIK_PARENT` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_MAGIK_PROCESS_LOCK` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/main.rs` |
| `MISTER_MAGIK_READY_FIFO` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/process_config.rs` |
| `MISTER_MAGIK_RELEASE_MARKER` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/update_checker.rs` |
| `MISTER_MAGIK_RETURN_TO_LAUNCHER` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_MAGIK_RUNTIME_DISPLAY_V1` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_display.rs` |
| `MISTER_MAGIK_RUNTIME_SETTINGS_V1` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_display.rs` |
| `MISTER_MAGIK_SOURCE_DIRTY` | build-time | path or path list | site-defined fallback; unchanged in P0 | build pipeline | `apps/mister/src/build_identity.rs` |
| `MISTER_MAGIK_SOURCE_REVISION` | build-time | string, enum, or boolean token | site-defined fallback; unchanged in P0 | build pipeline | `apps/mister/src/build_identity.rs` |
| `MISTER_MAGIK_STARTUP_TOKEN` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/process_config.rs` |
| `MISTER_MAGIK_TEST_AUTO_LAUNCH_GATE` | test | string, enum, or boolean token | site-defined fallback; unchanged in P0 | test only | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_MAGIK_TEST_CATALOG_PUBLICATION_GATE` | test | string, enum, or boolean token | site-defined fallback; unchanged in P0 | test only | `apps/mister/src/ui_runner/launcher_catalog_publication_test.rs` |
| `MISTER_MAGIK_TEST_CATALOG_PUBLICATION_SESSION` | test | string, enum, or boolean token | site-defined fallback; unchanged in P0 | test only | `apps/mister/src/ui_runner/launcher_catalog_publication_test.rs` |
| `MISTER_MAGIK_TEST_CATALOG_RECOVERY_DIALOG` | test | string, enum, or boolean token | site-defined fallback; unchanged in P0 | test only | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_MAGIK_TEST_FIRST_FRAME_RELEASE_GATE` | test | string, enum, or boolean token | site-defined fallback; unchanged in P0 | test only | `apps/mister/src/ui_runner/launcher_catalog_publication_test.rs` |
| `MISTER_MAGIK_TEST_LIBRARY_CHANGED_DIALOG_CHOICE` | test | string, enum, or boolean token | site-defined fallback; unchanged in P0 | test only | `apps/mister/src/launcher.rs` |
| `MISTER_MAGIK_VERSION` | build-time | string, enum, or boolean token | site-defined fallback; unchanged in P0 | build pipeline | `apps/mister/src/build_identity.rs` |
| `MISTER_MAME_SQLITE` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/catalog_config.rs` |
| `MISTER_MEDIA_ASSET_DIR` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/launcher.rs` |
| `MISTER_MEDIA_BENCH_CONTENTION` | benchmark | string, enum, or boolean token | site-defined fallback; unchanged in P0 | benchmark only | `apps/mister/src/launcher_runtime/media.rs` |
| `MISTER_MEDIA_CONCURRENCY` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/launcher_runtime/media.rs` |
| `MISTER_MEDIA_MANIFEST_URL` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/launcher_runtime/media.rs` |
| `MISTER_MEDIA_SIZE` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/launcher_runtime/media.rs` |
| `MISTER_MEDIA_UPDATE` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/launcher_runtime/media.rs` |
| `MISTER_MEGADRIVE_PREVIEW_ARCHIVE` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_MODE_FORMAT` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `mister/platform/runtime/src/framebuffer/format.rs` |
| `MISTER_N64_PREVIEW_ARCHIVE` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_NAMESPACE_BACKEND` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/library_indexer.rs` |
| `MISTER_NEOGEO_PREVIEW_ARCHIVE` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_NES_PREVIEW_ARCHIVE` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_ORIENTATION_PMU_COMPLETE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_ORIENTATION_SIMD` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/launcher_runtime/orientation_transition.rs` |
| `MISTER_ORIENTATION_TRANSITIONS_BENCHMARK` | benchmark | string, enum, or boolean token | site-defined fallback; unchanged in P0 | benchmark only | `apps/mister/src/ui_runner.rs` |
| `MISTER_ORIENTATION_TRANSITIONS_EVIDENCE_DIR` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_ORIENTATION_TRANSITIONS_REQUIRE_ANALYTICS` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_ORIENTATION_TRANSITION_EFFECT` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_PARTICLE_COHORTS` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `crates/particles/src/engine.rs` |
| `MISTER_PARTICLE_COMMAND_ORDER` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/particle_renderer.rs` |
| `MISTER_PARTICLE_PMU` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/particle_renderer.rs` |
| `MISTER_PARTICLE_PROJECTION` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `crates/particles/src/engine.rs` |
| `MISTER_PARTICLE_PROJECTION_VALIDATE` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `crates/particles/src/engine.rs` |
| `MISTER_PARTICLE_SIMD` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `crates/particles/src/engine.rs` |
| `MISTER_PMU_PROFILE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `crates/perf-events/src/lib.rs` |
| `MISTER_PMU_RECORD_LIMIT` | diagnostic | bounded integer | site-defined fallback; unchanged in P0 | developer diagnostic | `crates/perf-events/src/lib.rs` |
| `MISTER_PMU_SAMPLE_EVERY` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `crates/perf-events/src/lib.rs` |
| `MISTER_PPROF` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/cpu_profile.rs` |
| `MISTER_PPROF_COMPLETE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/cpu_profile.rs` |
| `MISTER_PPROF_DURATION_SECS` | diagnostic | bounded integer | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/cpu_profile.rs` |
| `MISTER_PPROF_FOLDED_OUT` | diagnostic | path or path list | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/cpu_profile.rs` |
| `MISTER_PPROF_HZ` | diagnostic | bounded integer | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/cpu_profile.rs` |
| `MISTER_PPROF_OUT` | diagnostic | path or path list | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/cpu_profile.rs` |
| `MISTER_PPROF_TRIGGER` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/cpu_profile.rs` |
| `MISTER_PPROF_WARMUP_SECS` | diagnostic | bounded integer | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/cpu_profile.rs` |
| `MISTER_PRESENT_BACKEND` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_PREVIEW_ARCHIVE` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_PREVIEW_ARCHIVES` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_PREVIEW_ARCHIVE_AUTO` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_PREVIEW_ARCHIVE_BACKGROUND_WARM` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_PREVIEW_ARCHIVE_MEM_PRIMARY` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_PREVIEW_ARCHIVE_MEM_WARM` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_PREVIEW_CACHE_DIR` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/device_layout.rs` |
| `MISTER_PREVIEW_DECODED_CACHE_CAP` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_PREVIEW_DIRECT_PRESENT` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/ui_frame_target.rs` |
| `MISTER_PREVIEW_FADE_P02` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/raw565_preview_renderer.rs` |
| `MISTER_PREVIEW_FORCE_ARCHIVE_MEM` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_PREVIEW_LOADING` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/preview_state.rs` |
| `MISTER_PREVIEW_RESIZE` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_PREVIEW_RESIZE_FILTER` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_PREVIEW_RESIZE_MAX` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_PREVIEW_RUN_LABEL` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/ui_frame_target.rs` |
| `MISTER_PREVIEW_SCROLL_EXIT_AFTER_TRACE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_PREVIEW_SCROLL_SKIP_ARCHIVE_WARM` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_PREVIEW_SCROLL_TRACE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launcher_frame_accounting.rs` |
| `MISTER_PREVIEW_SCROLL_TRACE_COMPLETE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launcher_frame_accounting.rs` |
| `MISTER_PREVIEW_SCROLL_TRACE_SECS` | diagnostic | bounded integer | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launcher_frame_accounting.rs` |
| `MISTER_PREVIEW_STEP_HOLD_SECS` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_bench.rs` |
| `MISTER_PREVIEW_TRACE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/preview_state.rs` |
| `MISTER_PREVIEW_TRANSITION` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/screenshot_transitions.rs` |
| `MISTER_PREVIEW_TRANSITION_MS` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/screenshot_transitions.rs` |
| `MISTER_PREVIEW_TRANSITION_PICKER` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/screenshot_transitions.rs` |
| `MISTER_PREVIEW_TRANSITION_SEGMENT_SECS` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/screenshot_transitions.rs` |
| `MISTER_PREVIEW_TURBO_LOOKAHEAD` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/preview_state.rs` |
| `MISTER_PREVIEW_TURBO_RUNWAY` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/preview_state.rs` |
| `MISTER_PREVIEW_VISUAL_PCT` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/preview_state.rs` |
| `MISTER_PROCESS_NAMES` | external | string, enum, or boolean token | site-defined fallback; unchanged in P0 | external compatibility | `mister/platform/runtime/src/main_command.rs` |
| `MISTER_PROFILE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/frame_profile.rs` |
| `MISTER_PROFILE_FILE` | diagnostic | path or path list | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/frame_profile.rs` |
| `MISTER_PROFILE_SLOW_US` | diagnostic | bounded integer | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/frame_profile.rs` |
| `MISTER_RASTER_EFFECTS` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/raster_effects_loop.rs` |
| `MISTER_RASTER_EFFECTS_AUTO` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/raster_effects_loop.rs` |
| `MISTER_RASTER_EFFECTS_CACHE_CAP` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/raster_effects_loop.rs` |
| `MISTER_RASTER_EFFECTS_HUD` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/raster_effects_loop.rs` |
| `MISTER_RASTER_EFFECTS_SEGMENT_SECS` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/raster_effects_loop.rs` |
| `MISTER_RASTER_EFFECTS_TRACE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/experiments/effects/raster_effects_loop.rs` |
| `MISTER_ROOT` | external | path or path list | site-defined fallback; unchanged in P0 | external compatibility | `apps/mister/src/macos_preview_content.rs` |
| `MISTER_SATURN_PREVIEW_ARCHIVE` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_SCREENSAVER_SEED` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_screensaver.rs` |
| `MISTER_SCREENSAVER_START_ACTIVE` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_SCREENSAVER_START_IDLE_WHEN_READY` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_SCREENSAVER_START_PREVIEW_AFTER_ANALYTICS` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_SCREENSAVER_START_PREVIEW_WHEN_READY` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_SETTINGS_NAVIGATION_BENCHMARK` | benchmark | string, enum, or boolean token | site-defined fallback; unchanged in P0 | benchmark only | `apps/mister/src/ui_runner.rs` |
| `MISTER_SETTINGS_NAVIGATION_EVIDENCE_DIR` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_SHARDED_CATALOG_DIR` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/pmu_profile.rs` |
| `MISTER_SMS_PREVIEW_ARCHIVE` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_SNES_PREVIEW_ARCHIVE` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/preview_worker.rs` |
| `MISTER_SPRITE_EFFECTS` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/sprite_effects_loop.rs` |
| `MISTER_SPRITE_EFFECTS_AUTO` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/sprite_effects_loop.rs` |
| `MISTER_SPRITE_EFFECTS_CACHE_CAP` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/sprite_effects_loop.rs` |
| `MISTER_SPRITE_EFFECTS_HUD` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/sprite_effects_loop.rs` |
| `MISTER_SPRITE_EFFECTS_SEGMENT_SECS` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/sprite_effects_loop.rs` |
| `MISTER_SPRITE_EFFECTS_TRACE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/experiments/effects/sprite_effects_loop.rs` |
| `MISTER_START_TIMEOUT` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/launcher.rs` |
| `MISTER_STREAM_SCALAR_BENCH_SAMPLES` | benchmark | string, enum, or boolean token | site-defined fallback; unchanged in P0 | benchmark only | `mister/platform/runtime/src/framebuffer/downsample.rs` |
| `MISTER_SYSTEM_ENTRY_BENCHMARK_SYSTEM` | benchmark | string, enum, or boolean token | site-defined fallback; unchanged in P0 | benchmark only | `apps/mister/src/ui_runner/launcher_bench.rs` |
| `MISTER_SYSTEM_ENTRY_PROFILE_OUT` | diagnostic | path or path list | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_SYSTEM_ENTRY_RUN_ID` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_SYSTEM_ENTRY_TRACE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/launcher_loop.rs` |
| `MISTER_TEST_EFFECTS_UNSET` | test | string, enum, or boolean token | site-defined fallback; unchanged in P0 | test only | `apps/mister/src/ui_runner/experiments/effects/effect_loop_support.rs` |
| `MISTER_TEXT_EFFECTS` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/text_effects_loop.rs` |
| `MISTER_TEXT_EFFECTS_AUTO` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/text_effects_loop.rs` |
| `MISTER_TEXT_EFFECTS_CACHE_CAP` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/text_effects_loop.rs` |
| `MISTER_TEXT_EFFECTS_HUD` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/text_effects_loop.rs` |
| `MISTER_TEXT_EFFECTS_SEGMENT_SECS` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/text_effects_loop.rs` |
| `MISTER_TEXT_EFFECTS_TRACE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/experiments/effects/text_effects_loop.rs` |
| `MISTER_THREAD_POLICY` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/runtime_thread.rs` |
| `MISTER_TRACE_FILE` | diagnostic | path or path list | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/frame_profile.rs` |
| `MISTER_TRANSITION_EFFECTS` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/transition_effects_loop.rs` |
| `MISTER_TRANSITION_EFFECTS_AUTO` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/transition_effects_loop.rs` |
| `MISTER_TRANSITION_EFFECTS_CACHE_CAP` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/transition_effects_loop.rs` |
| `MISTER_TRANSITION_EFFECTS_HUD` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/transition_effects_loop.rs` |
| `MISTER_TRANSITION_EFFECTS_SEGMENT_SECS` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/experiments/effects/transition_effects_loop.rs` |
| `MISTER_TRANSITION_EFFECTS_TRACE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/ui_runner/experiments/effects/transition_effects_loop.rs` |
| `MISTER_UI_FB_SIZE` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_display.rs` |
| `MISTER_USER_STATE_SQLITE` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `crates/catalog/src/catalog_config.rs` |
| `MISTER_VIDEO_AUTO_TOGGLE_MS` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/video_loop.rs` |
| `MISTER_VIDEO_DIR` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/video_player.rs` |
| `MISTER_VIDEO_PATH` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/video_player.rs` |
| `MISTER_VIDEO_PROFILE` | diagnostic | string, enum, or boolean token | site-defined fallback; unchanged in P0 | developer diagnostic | `apps/mister/src/frame_profile.rs` |
| `MISTER_VIDEO_SCALE` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `apps/mister/src/ui_runner/video_loop.rs` |
| `MISTER_VSYNC_DEGRADED_MISSES` | production | string, enum, or boolean token | site-defined fallback; unchanged in P0 | internal runtime | `mister/platform/runtime/src/framebuffer/vsync.rs` |
| `MISTER_VSYNC_DIRECT_WAIT` | production | path or path list | site-defined fallback; unchanged in P0 | internal runtime | `mister/platform/runtime/src/framebuffer/vsync.rs` |
| `MISTER_VSYNC_FALLBACK_HZ` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `mister/platform/runtime/src/framebuffer/vsync.rs` |
| `MISTER_VSYNC_FRESH_HIT_MAX_AGE_US` | production | bounded integer | site-defined fallback; unchanged in P0 | internal runtime | `mister/platform/runtime/src/framebuffer/vsync.rs` |
