# Device agent

Start with `src/main.rs`; scanout ABI handling uses `scanout_slots_contract.rs`.
Validate lengths, paths, and decoded sizes before allocation or I/O. Keep
non-Linux tests functional and OS access isolated from validation. Never expose
credentials or unauthenticated commands. Proxy framebuffer streams from the
producer instead of polling raw fb0.
