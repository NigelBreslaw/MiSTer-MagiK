# First-version review corrections

This closes the review of the original prototype (`c616f0662`, rebased as
`bd0d86301`). Scope remains the disposable probe and independent 2.0 tooling.
The real MagiK application, Main, FPGA and legacy orchestrators are unchanged.
Hardware evidence and measured limitations are recorded in [acceptance](../ACCEPTANCE.md).

| Review item | Correction | Verification |
|---|---|---|
| C01 Running identity | Published and running hashes are separate. Readiness binds PID, executable hash and first presentation. Starts carry the expected hash and reject a superseded upload before touching Main. Process records include Linux birth identity and actual executable identity. | Interrupted deployment and interleaved artifact tests; hardware superseded-start check. |
| C02 Capabilities | Each command requests only the capabilities it uses. Extra capabilities never trigger replacement; status and deployment do not require testing support. | Per-operation and missing-capability host tests. |
| C03 Reproducible builds | Versioned ARM build recipe, automatic image/container preparation, checkout-specific mounts, automatic agent builds and shared dependency downloads. | New local container/image prepared automatically; agent and probe ARM builds; mount ownership tests. |
| C04 Build cache | Fingerprint follows local Cargo dependencies, Slint imports, compiler configuration and flags. Generated outputs are excluded. Cached output bytes are verified; flags reach the container. One probe cache serves build and deploy. | Shared/configuration input, output exclusion, replaced artifact and flag forwarding tests; warm delivery matrix. |
| C05 Branch compatibility | Native credentials use shared user state. A missing local token discovers the installed token before deciding whether replacement is needed. Native update acknowledgement loss is reconciled by bounded polling, without blindly uploading again. | A→B→A capability/credential tests, slow update and lost acknowledgement tests; actual native updates retain the running probe. |
| C06 Lifecycle | Main FIFO exchange and observed launcher recovery are bounded. Ownership uses process birth identity; shutdown confirms exit and retains failure evidence. Readiness does not repeatedly hash the executable. | Missing FIFO reader test, ownership/readiness tests; hardware stop and observed Main recovery. |
| C07 Session cleanup | Host cleanup begins before Slint attachment. Native cleanup covers startup failure, relay failure and disconnect, restoring the same persistent artifact. Primary and cleanup failures remain visible. Viewer close shuts down its native connection. | Partial attachment tests, relay deadline/disconnect tests, viewer shutdown test; hardware disconnected-session recovery. |
| C08 Shared runner | Consumer scenarios are pytest tests with a small native fixture. Five separate uninstrumented 2-second warmup/5-second measurement repetitions share assertions with benchmarking. A separate optional 10-second profile repetition is labeled instrumented. | Pytest collection, smoke, five repetitions, separate profile, viewer-on repetitions. |
| C09 Honest measurements | Device windows retain deltas, physical drop availability, latch rejection, render time and render-to-presentation time separately. Exact build hash is accessible in the UI and measurement evidence. | Warmup/drop evidence unit test and hardware scenario assertions. |
| C10 Profile provenance | Unique profile directories reject reuse. Structured stack data produces folded stacks and SVG; completion requires nonzero samples and matching run/artifact identity. Keep at most twenty completed device profiles. | Stale/incomplete profile rejection tests; fresh hardware profile with application and renderer symbols. |
| C11 Preview isolation | Render thread publishes to one replaceable slot with a nonblocking lock. A worker performs socket I/O with a timeout; retained frame and log state is bounded. | Stalled producer-slot and actual Unix receiver tests; hardware viewer-on motion and stalled viewer/control checks. |
| C12 Upload bounds | Authenticate before the body; stream bounded chunks into a temporary file with incremental hashing, deadlines and atomic publication. Limit concurrent connections and payload size. | Truncation/authentication/hash/replay tests; repeated changed-binary deliveries and rejected bad upload on hardware. |
| C13 Evidence | Every command has real logs, timestamps, outcome, exact artifact identity, build fingerprint and agent identity where available. Diagnostics retain probe/agent output and Main status. Typed acceptance retains every attempt and restores sources/app. | Result/failure tests; reproducible delivery and recovery matrices with indexed bundles. |
| C14 Ownership and handoff | Canonical plans are in the repository. CODEOWNERS names tooling ownership; CI checks dedicated core changes separately from consumer edits. CI runs host and native tests. Viewer HTML/CSS/JS are separate readable assets. | Scope checker tests and local focused validation. Hosted CI remains authoritative after push. |

## Boundaries of the evidence

Mocked compatibility and build fixtures exercise branch-specific inputs and
capability subsets; they do not constitute a physical second MiSTer or an
independent developer workstation. The container was freshly created locally,
with dependency downloads already available. Destructive reboot/reset tests and
production application delivery are outside this correction milestone.

The native test relay has a 60-second session budget and a 20-second application
connection limit. Cleanup and Main recovery have their own bounded deadlines;
the relay deadline is not a promise that every recovery completes at exactly
60 seconds. Errors remain actionable and do not trigger reboot or rollback ladders.

Delivery timing targets are diagnostics, not deployment gates. The current delivery matrix uses two attempts per case, retaining failures and
reporting the slower result. Historical acceptance used twenty-sample p95.
