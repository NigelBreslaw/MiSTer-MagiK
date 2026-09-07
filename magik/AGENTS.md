# MiSTer MagiK tooling

Keep Python orchestration in `host/magik`, the Rust device service in `agent`,
and Mini-MagiK in `probe`. Shared app instrumentation belongs in
`crates/tooling-support`. Reuse capabilities, framing and delivery across consumers.
Do not require matching build identifiers or add automatic device test matrices.

Use typed `scripts/magik` operations with the root device rules. Capture MCP images
are agent input: explicitly embed the same image when the user asks to see it.
Do not recapture just to display it, or commit screenshot files.
