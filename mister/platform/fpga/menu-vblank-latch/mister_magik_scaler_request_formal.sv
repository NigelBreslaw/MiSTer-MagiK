// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
module request_formal;
 reg clk=0;
 always @($global_clock)clk<=!clk;
 (* anyseq *) reg reset_n,avl_step,o_step,schedule,accept_ready;
 wire issue_event,receive_event,accept_event,ack_event;
 wire [12:0] state;
 request_transport dut(.*);
 reg past_valid=0;
 reg [2:0] issued=0,received=0,accepted=0,acknowledged=0;
 reg stopped_seen=0,high_reset_seen=0,recovery_seen=0;
 wire [2:0] forward_count=issued-received;
 wire [2:0] pending_count=received-accepted;
 wire [2:0] reverse_count=accepted-acknowledged;
 wire [2:0] total_count=issued-acknowledged;
 wire [2:0] forward_tokens={2'b0,state[7]^state[6]}+
  {2'b0,state[6]^state[5]}+{2'b0,state[5]^state[4]}+{2'b0,state[4]^received[0]};
 wire [2:0] reverse_tokens={2'b0,state[3]^state[2]}+
  {2'b0,state[2]^state[1]}+{2'b0,state[1]^acknowledged[0]};
 (* keep *) wire witnessed=stopped_seen && high_reset_seen && recovery_seen;
 always @(posedge clk or negedge reset_n)begin
  if(!reset_n)begin issued<=0;received<=0;accepted<=0;acknowledged<=0;end
  else begin
   issued<=issued+issue_event;received<=received+receive_event;
   accepted<=accepted+accept_event;acknowledged<=acknowledged+ack_event;
  end
 end
 always @($global_clock)begin
  past_valid<=1;
  if(!past_valid)assume(!reset_n);
  if(past_valid && reset_n)begin
   assert(forward_count<=1);
   assert(pending_count<=1);
   assert(reverse_count<=1);
   assert(total_count<=1);
   assert(total_count=={2'b0,state[9]});
   assert(state[12]==(state[4]^received[0]));
   assert(issued[0]==state[7]);
   assert(accepted[0]==state[3]);
   assert(forward_count==forward_tokens);
   assert(pending_count=={2'b0,state[8]});
   assert(reverse_count==reverse_tokens);
   assert(state[0]==(state[1]^acknowledged[0]));
   assert(!receive_event || forward_count==1);
   assert(!accept_event || pending_count==1);
   assert(!ack_event || reverse_count==1);
   assert(!issue_event || total_count==0);
  end
  if(past_valid && reset_n && !o_step && pending_count==1)stopped_seen<=1;
  if(past_valid && !reset_n && $past(state[7]))high_reset_seen<=1;
  if(high_reset_seen && reset_n && ack_event)recovery_seen<=1;
 end
endmodule
