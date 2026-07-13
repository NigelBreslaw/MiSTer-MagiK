#include "Vtb_mister_magik_vblank_latch.h"
#include "verilated.h"
#include "verilated_cov.h"

int main(int argc, char** argv) {
    VerilatedContext context;
    context.commandArgs(argc, argv);
    Vtb_mister_magik_vblank_latch model{&context};
    while (!context.gotFinish()) {
        model.eval();
        if (!model.eventsPending()) break;
        context.time(model.nextTimeSlot());
    }
    model.final();
    context.coveragep()->write("coverage.dat");
    return context.gotFinish() ? 0 : 1;
}
