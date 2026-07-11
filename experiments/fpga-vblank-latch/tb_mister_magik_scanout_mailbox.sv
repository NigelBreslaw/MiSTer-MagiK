`timescale 1ns/1ps

module tb_mister_magik_scanout_mailbox;
	localparam [31:0] BASE = 32'h00123000;
	localparam [31:0] EPOCH = 32'h10203040;

	reg clk = 0;
	reg reset = 1;
	reg enable = 0;
	reg vblank = 0;
	always #5 clk = ~clk;

	wire apply;
	wire post_enable;
	wire post_filter;
	wire [5:0] post_format;
	wire [1:0] post_slot;
	wire [31:0] post_base;
	wire [11:0] post_width;
	wire [11:0] post_height;
	wire [13:0] post_stride;
	wire [11:0] post_hmin, post_hmax, post_vmin, post_vmax;
	wire [31:0] active_sequence, pending_sequence;
	wire pending;
	wire [15:0] apply_count, error_count;

	wire [7:0] awid, wid, arid;
	wire [31:0] awaddr, araddr;
	wire [3:0] awlen, arlen, awcache, arcache;
	wire [2:0] awsize, arsize, awprot, arprot;
	wire [1:0] awburst, arburst, awlock, arlock;
	wire awvalid, wvalid, bready, arvalid, rready;
	wire [4:0] awuser, aruser;
	wire [127:0] wdata;
	wire [15:0] wstrb;
	wire wlast;
	reg awready = 1;
	reg wready = 1;
	reg [7:0] bid = 0;
	reg [1:0] bresp = 0;
	reg bvalid = 0;
	reg arready = 1;
	reg [7:0] rid = 0;
	reg [127:0] rdata = 0;
	reg [1:0] rresp = 0;
	reg rlast = 1;
	reg rvalid = 0;

	reg [127:0] control;
	reg [127:0] descriptor_a0;
	reg [127:0] descriptor_a1;
	reg [127:0] descriptor_b0;
	reg [127:0] descriptor_b1;
	reg [127:0] completion;
	reg read_queued = 0;
	reg [31:0] read_address = 0;
	reg got_aw = 0;
	reg got_w = 0;
	reg [31:0] write_address = 0;
	reg [127:0] write_data = 0;

	mister_magik_scanout_mailbox #(.POLL_CYCLES(4)) dut (
		.clk(clk), .reset(reset), .enable(enable),
		.mailbox_base(BASE), .mailbox_epoch(EPOCH), .vblank(vblank),
		.apply(apply), .post_enable(post_enable), .post_filter(post_filter),
		.post_format(post_format), .post_slot(post_slot), .post_base(post_base),
		.post_width(post_width), .post_height(post_height),
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

	function automatic [127:0] read_line(input [31:0] address);
		begin
			case (address - BASE)
				32'h00: read_line = control;
				32'h40: read_line = descriptor_a0;
				32'h50: read_line = descriptor_a1;
				32'h80: read_line = descriptor_b0;
				32'h90: read_line = descriptor_b1;
				default: read_line = 128'd0;
			endcase
		end
	endfunction

	always @(posedge clk) begin
		if (arvalid && arready) begin
			read_queued <= 1;
			read_address <= araddr;
		end
		if (read_queued && !rvalid) begin
			read_queued <= 0;
			rid <= arid;
			rdata <= read_line(read_address);
			rvalid <= 1;
		end
		if (rvalid && rready)
			rvalid <= 0;

		if (awvalid && awready) begin
			got_aw <= 1;
			write_address <= awaddr;
			bid <= awid;
		end
		if (wvalid && wready) begin
			got_w <= 1;
			write_data <= wdata;
		end
		if (got_aw && got_w && !bvalid) begin
			got_aw <= 0;
			got_w <= 0;
			if (write_address == BASE + 32'hc0)
				completion <= write_data;
			bvalid <= 1;
		end
		if (bvalid && bready)
			bvalid <= 0;
	end

	task automatic wait_pending(input [31:0] expected_sequence);
		integer cycles;
		begin
			cycles = 0;
			while ((!pending || pending_sequence != expected_sequence) && cycles < 200) begin
				@(posedge clk);
				cycles = cycles + 1;
			end
			if (!pending || pending_sequence != expected_sequence)
				$fatal(1, "sequence %0d was not staged", expected_sequence);
		end
	endtask

	task automatic apply_at_vblank(input [31:0] expected_sequence);
		begin
			@(negedge clk); vblank <= 1;
			repeat (4) @(negedge clk);
			vblank <= 0;
			wait (active_sequence == expected_sequence);
			wait (completion[31:0] == 32'h4d47434d &&
			      completion[95:64] == expected_sequence);
			if (completion[63:32] != EPOCH)
				$fatal(1, "sequence %0d completion used the wrong epoch", expected_sequence);
		end
	endtask

	initial begin
		control = {31'd0, 1'b0, 32'd1, EPOCH, 32'h4d475343};
		descriptor_a0 = {32'h02000000, 32'd1, EPOCH, 32'h4d474452};
		descriptor_a1 = {4'd0, 12'd539, 4'd0, 12'd0,
		               4'd0, 12'd959, 4'd0, 12'd0,
		               2'd0, 14'd1920, 4'd0, 12'd540,
		               4'd0, 12'd960, 6'd0, 2'd2, 1'b1, 1'b0, 6'd4};
		descriptor_b0 = {32'h03000000, 32'd0, EPOCH, 32'h4d474452};
		descriptor_b1 = {4'd0, 12'd539, 4'd0, 12'd0,
		               4'd0, 12'd959, 4'd0, 12'd0,
		               2'd0, 14'd1920, 4'd0, 12'd540,
		               4'd0, 12'd960, 6'd0, 2'd1, 1'b1, 1'b0, 6'd4};
		completion = 0;

		repeat (4) @(posedge clk);
		reset <= 0;
		enable <= 1;
		if (awcache != 4'b1111 || arcache != 4'b1111 ||
		    awuser != 5'b11111 || aruser != 5'b11111)
			$fatal(1, "ACP transactions are not coherent cacheable accesses");

		wait_pending(1);
		if (!post_enable || post_filter || post_format != 4 || post_slot != 2 ||
		    post_base != 32'h02000000 || post_width != 960 || post_height != 540 ||
		    post_stride != 1920 || post_hmax != 959 || post_vmax != 539)
			$fatal(1, "descriptor fields were decoded incorrectly");

		apply_at_vblank(1);

		// Publish a torn descriptor. The control line changes before its
		// descriptor, so the first poll must reject it. Once both match the
		// stable re-read accepts sequence 2.
		control = {31'd0, 1'b1, 32'd2, EPOCH, 32'h4d475343};
		repeat (80) @(posedge clk);
		if (pending)
			$fatal(1, "torn descriptor was staged");
		descriptor_b0[95:64] = 2;
		wait_pending(2);
		if (post_slot != 1 || post_base != 32'h03000000)
			$fatal(1, "descriptor B was not selected for sequence 2");
		apply_at_vblank(2);

		// Reuse descriptor A only after B completed, matching the kernel's
		// alternating descriptor-bank protocol.
		descriptor_a0 = {32'h04000000, 32'd3, EPOCH, 32'h4d474452};
		descriptor_a1[9:8] = 2'd2;
		control = {31'd0, 1'b0, 32'd3, EPOCH, 32'h4d475343};
		wait_pending(3);
		if (post_slot != 2 || post_base != 32'h04000000)
			$fatal(1, "descriptor A was not reused for sequence 3");
		apply_at_vblank(3);

		if (apply_count != 3 || error_count != 0)
			$fatal(1, "unexpected AXI errors: %0d", error_count);
		$display("PASS: coherent ACP attributes, B/A/B staging, vblank apply, completion, tear rejection");
		$finish;
	end
endmodule

// Signature-only simulation stand-in for the Cyclone V hard primitive. This
// lets the bridge wrapper elaborate in Icarus; Quartus supplies the real cell.
module cyclonev_hps_interface_fpga2hps (
	input [1:0] port_size_config, input clk,
	input [7:0] awid, input [31:0] awaddr, input [3:0] awlen,
	input [2:0] awsize, input [1:0] awburst, input [1:0] awlock,
	input [3:0] awcache, input [2:0] awprot, input awvalid,
	output awready, input [4:0] awuser, input [7:0] wid,
	input [127:0] wdata, input [15:0] wstrb, input wlast, input wvalid,
	output wready, output [7:0] bid, output [1:0] bresp, output bvalid,
	input bready, input [7:0] arid, input [31:0] araddr,
	input [3:0] arlen, input [2:0] arsize, input [1:0] arburst,
	input [1:0] arlock, input [3:0] arcache, input [2:0] arprot,
	input arvalid, output arready, input [4:0] aruser,
	output [7:0] rid, output [127:0] rdata, output [1:0] rresp,
	output rlast, output rvalid, input rready
);
	assign awready = 1'b0;
	assign wready = 1'b0;
	assign bid = 8'd0;
	assign bresp = 2'd0;
	assign bvalid = 1'b0;
	assign arready = 1'b0;
	assign rid = 8'd0;
	assign rdata = 128'd0;
	assign rresp = 2'd0;
	assign rlast = 1'b0;
	assign rvalid = 1'b0;
endmodule
