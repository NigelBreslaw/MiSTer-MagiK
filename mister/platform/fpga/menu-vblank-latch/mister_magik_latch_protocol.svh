/* SPDX-License-Identifier: GPL-3.0-or-later */
/* Copyright (C) 2026 Nigel Breslaw */

localparam [7:0]  MAGIK_UIO_SET_FBUF_LATCH = 8'h57;
localparam [7:0]  MAGIK_UIO_GET_FBUF_LATCH = 8'h58;
localparam [7:0]  MAGIK_UIO_GET_FBUF_LATCH_CAPS = 8'h59;
localparam [15:0] MAGIK_FBUF_LATCH_MAGIC = 16'h4D47;
localparam [15:0] MAGIK_FBUF_STATUS_MAGIC = 16'h4D48;
localparam [15:0] MAGIK_FBUF_CAPS_MAGIC = 16'h4D49;
localparam [15:0] MAGIK_FBUF_PROTOCOL_VERSION = 16'd2;
localparam [15:0] MAGIK_FBUF_CAPS_FLAGS = 16'h0007;
localparam [15:0] MAGIK_FBUF_MAX_WIDTH = 16'd1366;
localparam [15:0] MAGIK_FBUF_MAX_HEIGHT = 16'd768;
localparam [15:0] MAGIK_FBUF_MAX_STRIDE = 16'd2736;
