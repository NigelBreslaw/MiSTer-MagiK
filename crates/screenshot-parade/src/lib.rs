// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Slint-free screenshot parade rendering shared by production and scene labs.

mod live;
mod raster;
mod reservoir;
mod schedule;
mod slack;

pub use live::{
    LiveScreenshotConfig, LiveScreenshotParade, LiveScreenshotPoll, ReadyScreenshotFrame,
    ScreenshotBuffer, ScreenshotRenderTiming, ScreenshotSequenceFailure,
};
pub use raster::{PARADE_SUBPIXEL_ONE, PreparedScreenshotCard, ScreenshotImage};
pub use reservoir::{
    STRICT_READY_CAPACITY, STRICT_RENDER_BUFFER_COUNT, StrictFrameConsumer, StrictFramePoll,
    StrictFrameProducer, StrictFreeBufferPoll, StrictReadyFrame, strict_render_reservoir,
};
pub use schedule::{
    ScreenshotParade, ScreenshotParadeConfig, ScreenshotParadeReplacementMode,
    ScreenshotParadeStartup, ScreenshotParadeStats, WorkerStartCallback,
};
pub use slack::{PreparationSlack, RenderPauseReceipt};
