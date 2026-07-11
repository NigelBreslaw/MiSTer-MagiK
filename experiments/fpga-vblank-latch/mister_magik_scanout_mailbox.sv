`timescale 1ns/1ps
// SPDX-License-Identifier: GPL-2.0-or-later
//
// MiSTer MagiK scanout mailbox.
//
// This master accesses one 4 KiB CPU/FPGA coherent mailbox through the
// Cyclone V FPGA-to-HPS bridge and ACP ID mapper. Pixel storage never travels
// through this port. All transfers are single 128-bit AXI3 beats.
module mister_magik_scanout_mailbox #(
	parameter integer POLL_CYCLES = 1024
) (
	input  wire         clk,
	input  wire         reset,
	input  wire         enable,
	input  wire [31:0]  mailbox_base,
	input  wire [31:0]  mailbox_epoch,
	input  wire         vblank,

	output reg          apply = 1'b0,
	output reg          post_enable = 1'b0,
	output reg          post_filter = 1'b0,
	output reg  [5:0]   post_format = 6'd0,
	output reg  [1:0]   post_slot = 2'd0,
	output reg  [31:0]  post_base = 32'd0,
	output reg  [11:0]  post_width = 12'd0,
	output reg  [11:0]  post_height = 12'd0,
	output reg  [13:0]  post_stride = 14'd0,
	output reg  [11:0]  post_hmin = 12'd0,
	output reg  [11:0]  post_hmax = 12'd0,
	output reg  [11:0]  post_vmin = 12'd0,
	output reg  [11:0]  post_vmax = 12'd0,

	output reg  [31:0]  active_sequence = 32'd0,
	output reg  [31:0]  pending_sequence = 32'd0,
	output reg          pending = 1'b0,
	output reg  [15:0]  apply_count = 16'd0,
	output reg  [15:0]  error_count = 16'd0,

	output reg  [7:0]   axi_awid = 8'h20,
	output reg  [31:0]  axi_awaddr = 32'd0,
	output wire [3:0]   axi_awlen,
	output wire [2:0]   axi_awsize,
	output wire [1:0]   axi_awburst,
	output wire [1:0]   axi_awlock,
	output wire [3:0]   axi_awcache,
	output wire [2:0]   axi_awprot,
	output reg          axi_awvalid = 1'b0,
	input  wire         axi_awready,
	output wire [4:0]   axi_awuser,
	output wire [7:0]   axi_wid,
	output reg  [127:0] axi_wdata = 128'd0,
	output wire [15:0]  axi_wstrb,
	output wire         axi_wlast,
	output reg          axi_wvalid = 1'b0,
	input  wire         axi_wready,
	input  wire [7:0]   axi_bid,
	input  wire [1:0]   axi_bresp,
	input  wire         axi_bvalid,
	output reg          axi_bready = 1'b0,
	output reg  [7:0]   axi_arid = 8'h20,
	output reg  [31:0]  axi_araddr = 32'd0,
	output wire [3:0]   axi_arlen,
	output wire [2:0]   axi_arsize,
	output wire [1:0]   axi_arburst,
	output wire [1:0]   axi_arlock,
	output wire [3:0]   axi_arcache,
	output wire [2:0]   axi_arprot,
	output reg          axi_arvalid = 1'b0,
	input  wire         axi_arready,
	output wire [4:0]   axi_aruser,
	input  wire [7:0]   axi_rid,
	input  wire [127:0] axi_rdata,
	input  wire [1:0]   axi_rresp,
	input  wire         axi_rlast,
	input  wire         axi_rvalid,
	output reg          axi_rready = 1'b0
);
	localparam [31:0] CONTROL_MAGIC    = 32'h4d475343; // "MGSC"
	localparam [31:0] DESCRIPTOR_MAGIC = 32'h4d474452; // "MGDR"
	localparam [31:0] COMPLETION_MAGIC = 32'h4d47434d; // "MGCM"
	localparam [31:0] DESC_A_OFFSET    = 32'h00000040;
	localparam [31:0] DESC_B_OFFSET    = 32'h00000080;
	localparam [31:0] COMPLETE_OFFSET  = 32'h000000c0;

	localparam [3:0] IDLE        = 4'd0;
	localparam [3:0] CTRL_ADDR   = 4'd1;
	localparam [3:0] CTRL_DATA   = 4'd2;
	localparam [3:0] DESC0_ADDR  = 4'd3;
	localparam [3:0] DESC0_DATA  = 4'd4;
	localparam [3:0] DESC1_ADDR  = 4'd5;
	localparam [3:0] DESC1_DATA  = 4'd6;
	localparam [3:0] VERIFY_ADDR = 4'd7;
	localparam [3:0] VERIFY_DATA = 4'd8;
	localparam [3:0] WRITE_SEND  = 4'd9;
	localparam [3:0] WRITE_RESP  = 4'd10;

	reg [3:0] state = IDLE;
	reg [31:0] poll_count = 32'd0;
	reg vblank_meta = 1'b0;
	reg vblank_sync = 1'b0;
	reg vblank_old = 1'b0;
	reg completion_due = 1'b0;
	reg aw_done = 1'b0;
	reg w_done = 1'b0;
	reg [31:0] candidate_sequence = 32'd0;
	reg candidate_index = 1'b0;
	reg [127:0] descriptor0 = 128'd0;
	reg [127:0] descriptor1 = 128'd0;

	assign axi_awlen   = 4'd0;
	assign axi_awsize  = 3'd4;
	assign axi_awburst = 2'b01;
	assign axi_awlock  = 2'b00;
	assign axi_awcache = 4'b1111;
	assign axi_awprot  = 3'b000;
	assign axi_awuser  = 5'b00001;
	assign axi_wid     = axi_awid;
	assign axi_wstrb   = 16'hffff;
	assign axi_wlast   = 1'b1;
	assign axi_arlen   = 4'd0;
	assign axi_arsize  = 3'd4;
	assign axi_arburst = 2'b01;
	assign axi_arlock  = 2'b00;
	assign axi_arcache = 4'b1111;
	assign axi_arprot  = 3'b000;
	assign axi_aruser  = 5'b00001;

	always @(posedge clk) begin
		apply <= 1'b0;
		vblank_meta <= vblank;
		vblank_sync <= vblank_meta;
		vblank_old <= vblank_sync;

		if (reset || !enable) begin
			state <= IDLE;
			poll_count <= 32'd0;
			pending <= 1'b0;
			completion_due <= 1'b0;
			active_sequence <= 32'd0;
			pending_sequence <= 32'd0;
			apply_count <= 16'd0;
			error_count <= 16'd0;
			axi_arvalid <= 1'b0;
			axi_rready <= 1'b0;
			axi_awvalid <= 1'b0;
			axi_wvalid <= 1'b0;
			axi_bready <= 1'b0;
			aw_done <= 1'b0;
			w_done <= 1'b0;
			vblank_meta <= 1'b0;
			vblank_sync <= 1'b0;
			vblank_old <= 1'b0;
		end else begin
			if (pending && vblank_sync && !vblank_old) begin
				apply <= 1'b1;
				active_sequence <= pending_sequence;
				pending <= 1'b0;
				completion_due <= 1'b1;
				apply_count <= apply_count + 1'd1;
			end

			case (state)
				IDLE: begin
					if (completion_due) begin
						axi_awaddr <= mailbox_base + COMPLETE_OFFSET;
						axi_wdata <= {
							25'd0, post_slot, post_enable, pending, 3'd0,
							active_sequence, mailbox_epoch, COMPLETION_MAGIC
						};
						axi_awvalid <= 1'b1;
						axi_wvalid <= 1'b1;
						aw_done <= 1'b0;
						w_done <= 1'b0;
						completion_due <= 1'b0;
						state <= WRITE_SEND;
					end else if (poll_count == 0) begin
						axi_araddr <= mailbox_base;
						axi_arvalid <= 1'b1;
						state <= CTRL_ADDR;
						poll_count <= POLL_CYCLES - 1;
					end else begin
						poll_count <= poll_count - 1'd1;
					end
				end

				CTRL_ADDR, DESC0_ADDR, DESC1_ADDR, VERIFY_ADDR: begin
					if (axi_arvalid && axi_arready) begin
						axi_arvalid <= 1'b0;
						axi_rready <= 1'b1;
						state <= state + 1'd1;
					end
				end

				CTRL_DATA: begin
					if (axi_rvalid) begin
						axi_rready <= 1'b0;
						if (axi_rresp != 2'b00 || !axi_rlast || axi_rid != axi_arid) begin
							error_count <= error_count + 1'd1;
							state <= IDLE;
						end else if (axi_rdata[31:0] == CONTROL_MAGIC &&
						             axi_rdata[63:32] == mailbox_epoch &&
						             axi_rdata[95:64] != active_sequence &&
						             !pending &&
						             axi_rdata[127:97] == 31'd0) begin
							candidate_sequence <= axi_rdata[95:64];
							candidate_index <= axi_rdata[96];
							axi_araddr <= mailbox_base + (axi_rdata[96] ? DESC_B_OFFSET : DESC_A_OFFSET);
							axi_arvalid <= 1'b1;
							state <= DESC0_ADDR;
						end else begin
							state <= IDLE;
						end
					end
				end

				DESC0_DATA: begin
					if (axi_rvalid) begin
						axi_rready <= 1'b0;
						if (axi_rresp != 2'b00 || !axi_rlast || axi_rid != axi_arid) begin
							error_count <= error_count + 1'd1;
							state <= IDLE;
						end else begin
							descriptor0 <= axi_rdata;
							axi_araddr <= axi_araddr + 32'd16;
							axi_arvalid <= 1'b1;
							state <= DESC1_ADDR;
						end
					end
				end

				DESC1_DATA: begin
					if (axi_rvalid) begin
						axi_rready <= 1'b0;
						if (axi_rresp != 2'b00 || !axi_rlast || axi_rid != axi_arid) begin
							error_count <= error_count + 1'd1;
							state <= IDLE;
						end else begin
							descriptor1 <= axi_rdata;
							axi_araddr <= mailbox_base;
							axi_arvalid <= 1'b1;
							state <= VERIFY_ADDR;
						end
					end
				end

				VERIFY_DATA: begin
					if (axi_rvalid) begin
						axi_rready <= 1'b0;
						if (axi_rresp != 2'b00 || !axi_rlast || axi_rid != axi_arid) begin
							error_count <= error_count + 1'd1;
						end else if (axi_rdata[31:0] == CONTROL_MAGIC &&
						             axi_rdata[63:32] == mailbox_epoch &&
						             axi_rdata[95:64] == candidate_sequence &&
						             axi_rdata[96] == candidate_index &&
						             descriptor0[31:0] == DESCRIPTOR_MAGIC &&
						             descriptor0[63:32] == mailbox_epoch &&
						             descriptor0[95:64] == candidate_sequence) begin
							post_base <= descriptor0[127:96];
							post_format <= descriptor1[5:0];
							post_filter <= descriptor1[6];
							post_enable <= descriptor1[7];
							post_slot <= descriptor1[9:8];
							post_width <= descriptor1[27:16];
							post_height <= descriptor1[43:32];
							post_stride <= descriptor1[61:48];
							post_hmin <= descriptor1[75:64];
							post_hmax <= descriptor1[91:80];
							post_vmin <= descriptor1[107:96];
							post_vmax <= descriptor1[123:112];
							pending_sequence <= candidate_sequence;
							pending <= 1'b1;
						end
						state <= IDLE;
					end
				end

				WRITE_SEND: begin
					if (axi_awvalid && axi_awready) begin
						axi_awvalid <= 1'b0;
						aw_done <= 1'b1;
					end
					if (axi_wvalid && axi_wready) begin
						axi_wvalid <= 1'b0;
						w_done <= 1'b1;
					end
					if ((aw_done || (axi_awvalid && axi_awready)) &&
					    (w_done || (axi_wvalid && axi_wready))) begin
						axi_bready <= 1'b1;
						state <= WRITE_RESP;
					end
				end

				WRITE_RESP: begin
					if (axi_bvalid) begin
						axi_bready <= 1'b0;
						if (axi_bresp != 2'b00 || axi_bid != axi_awid)
							error_count <= error_count + 1'd1;
						state <= IDLE;
					end
				end

				default: state <= IDLE;
			endcase
		end
	end
endmodule

// Keep the generated Cyclone V primitive wiring out of sys_top.v. Platform
// Designer encodes 128-bit FPGA-to-HPS as port_size_config=2'b10; 2'b11 is the
// disabled setting used by stock MiSTer Menu cores.
module mister_magik_scanout_mailbox_bridge (
	input  wire        clk,
	input  wire        reset,
	input  wire        enable,
	input  wire [31:0] mailbox_base,
	input  wire [31:0] mailbox_epoch,
	input  wire        vblank,
	output wire        apply,
	output wire        post_enable,
	output wire        post_filter,
	output wire [5:0]  post_format,
	output wire [1:0]  post_slot,
	output wire [31:0] post_base,
	output wire [11:0] post_width,
	output wire [11:0] post_height,
	output wire [13:0] post_stride,
	output wire [11:0] post_hmin,
	output wire [11:0] post_hmax,
	output wire [11:0] post_vmin,
	output wire [11:0] post_vmax,
	output wire [31:0] active_sequence,
	output wire [31:0] pending_sequence,
	output wire        pending,
	output wire [15:0] apply_count,
	output wire [15:0] error_count
);
	wire [7:0] awid, wid, bid, arid, rid;
	wire [31:0] awaddr, araddr;
	wire [3:0] awlen, arlen, awcache, arcache;
	wire [2:0] awsize, arsize, awprot, arprot;
	wire [1:0] awburst, arburst, awlock, arlock, bresp, rresp;
	wire awvalid, awready, wvalid, wready, bvalid, bready;
	wire arvalid, arready, rvalid, rready, rlast;
	wire [4:0] awuser, aruser;
	wire [127:0] wdata, rdata;
	wire [15:0] wstrb;
	wire wlast;

	mister_magik_scanout_mailbox mailbox (
		.clk(clk), .reset(reset), .enable(enable),
		.mailbox_base(mailbox_base), .mailbox_epoch(mailbox_epoch),
		.vblank(vblank), .apply(apply), .post_enable(post_enable),
		.post_filter(post_filter), .post_format(post_format), .post_slot(post_slot),
		.post_base(post_base), .post_width(post_width), .post_height(post_height),
		.post_stride(post_stride), .post_hmin(post_hmin), .post_hmax(post_hmax),
		.post_vmin(post_vmin), .post_vmax(post_vmax),
		.active_sequence(active_sequence), .pending_sequence(pending_sequence),
		.pending(pending), .apply_count(apply_count), .error_count(error_count),
		.axi_awid(awid), .axi_awaddr(awaddr), .axi_awlen(awlen),
		.axi_awsize(awsize), .axi_awburst(awburst), .axi_awlock(awlock),
		.axi_awcache(awcache), .axi_awprot(awprot), .axi_awvalid(awvalid),
		.axi_awready(awready), .axi_awuser(awuser), .axi_wid(wid),
		.axi_wdata(wdata), .axi_wstrb(wstrb), .axi_wlast(wlast),
		.axi_wvalid(wvalid), .axi_wready(wready), .axi_bid(bid),
		.axi_bresp(bresp), .axi_bvalid(bvalid), .axi_bready(bready),
		.axi_arid(arid), .axi_araddr(araddr), .axi_arlen(arlen),
		.axi_arsize(arsize), .axi_arburst(arburst), .axi_arlock(arlock),
		.axi_arcache(arcache), .axi_arprot(arprot), .axi_arvalid(arvalid),
		.axi_arready(arready), .axi_aruser(aruser), .axi_rid(rid),
		.axi_rdata(rdata), .axi_rresp(rresp), .axi_rlast(rlast),
		.axi_rvalid(rvalid), .axi_rready(rready)
	);

	cyclonev_hps_interface_fpga2hps fpga2hps (
		.port_size_config(2'b10), .clk(clk),
		.awid(awid), .awaddr(awaddr), .awlen(awlen), .awsize(awsize),
		.awburst(awburst), .awlock(awlock), .awcache(awcache),
		.awprot(awprot), .awvalid(awvalid), .awready(awready), .awuser(awuser),
		.wid(wid), .wdata(wdata), .wstrb(wstrb), .wlast(wlast),
		.wvalid(wvalid), .wready(wready), .bid(bid), .bresp(bresp),
		.bvalid(bvalid), .bready(bready),
		.arid(arid), .araddr(araddr), .arlen(arlen), .arsize(arsize),
		.arburst(arburst), .arlock(arlock), .arcache(arcache),
		.arprot(arprot), .arvalid(arvalid), .arready(arready), .aruser(aruser),
		.rid(rid), .rdata(rdata), .rresp(rresp), .rlast(rlast),
		.rvalid(rvalid), .rready(rready)
	);
endmodule
