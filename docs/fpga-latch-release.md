# FPGA latch-v3 release requirements

The latch is a production subsystem. Its source lives under
`mister/platform/fpga/menu-vblank-latch/`, and its installed RBF lives under
`/media/fat/mister-magik/fpga/`. Root `/media/fat/menu.rbf` is stock firmware
owned by `update_all`. Every new RBF hash must complete this qualification
before platform-manifest activation.

The production scope is commands `0x57` SET, `0x58` GET, and `0x59` CAPS plus
the vblank-latched framebuffer route. The retired scanout mailbox, command
`0x5a`, AXI/ACP access, DMA ownership, and completion fences are not part of
this design.

## Behavioral requirements

- **LATCH-V3-001 — discovery and capabilities.** The commands retain magic
  `0x4d47`, `0x4d48`, and `0x4d49`. CAPS advertises exact protocol 3, flags
  `0x007f`, limits 1366x768/2736, and a valid CRC.
- **LATCH-V3-002 — isolated receive staging.** SET words 0–10 enter a private
  receive bundle. Partial, duplicate, shifted, reordered, restarted, and
  post-close transactions cannot alter committed pending or active state.
- **LATCH-V3-003 — CRC commit.** SET word 11 is the sole commit point. It
  commits only a complete ordered mask with valid CRC and increments post count
  once.
- **LATCH-V3-004 — semantic validation.** Enabled routes require RGB565+RXB,
  legal mode bits, aligned nonzero base, bounded geometry/stride, ordered
  destination bounds, and non-wrapping widened address arithmetic. Disabled
  routes are canonical all-zero bundles apart from the opaque sequence.
- **LATCH-V3-005 — accepted vblank application.** A committed route applies
  only on synchronized rising vblank and explicit `apply_accepted` feedback.
  Preempted applies remain pending.
- **LATCH-V3-006 — replacement arbitration.** Pending replacement increments
  drop once. If old apply and new commit coincide, old applies, new remains
  pending, and no replacement drop is counted.
- **LATCH-V3-007 — coherent status.** GET snapshots all thirteen non-CRC words
  at command start and returns one coherent snapshot plus CRC despite apply or
  legacy activity during the read.
- **LATCH-V3-008 — active ownership.** Authoritative legacy Main `LFB_*` writes
  win same-edge arbitration, clear MagiK ownership/active sequence, and advance
  route epoch. Accepted MagiK applies restore ownership and advance epoch.
- **LATCH-V3-009 — rejection evidence.** A malformed transaction increments
  reject count once and publishes its four-bit reason without changing pending
  state.
- **LATCH-V3-010 — finite-width behavior.** Sequences are opaque 16-bit values.
  Post, flip, drop, reject, and epoch counters wrap modulo 65,536 without
  changing arbitration.
- **LATCH-V3-011 — transition compatibility.** New runtime may negotiate the
  frozen protocol-v2 RBF for interrupted upgrades and rollback. It never
  downgrades malformed v3 traffic. Every newly built FPGA release is v3.
- **LATCH-V3-012 — production integration.** The simulated command/strobe
  bridge is the exact SystemVerilog module copied into the RBF. All three
  opcodes are checked for upstream conflicts and the complete `sys_top` apply
  bundle is structurally verified.

## Requirement-to-test matrix

Retained results must identify the exact source, component, RBF, and report
hashes under test.

| Requirement | Deterministic gate | Hardware evidence |
| --- | --- | --- |
| LATCH-V3-001 | Exact CAPS payload and golden CRC | Passive report shows exact v3 profile |
| LATCH-V3-002/003 | Vblank after every SET word; corrupt every word/CRC | No partial active geometry or pending corruption |
| LATCH-V3-004 | Every semantic rejection and canonical disabled route | Deliberate invalid posts reject once and recover |
| LATCH-V3-005/006 | Apply/commit, apply/reject, replacement, falling/no-edge cases | Flip/post/drop deltas match reference behavior |
| LATCH-V3-007 | Route change after every GET word; snapshot CRC | Status agrees with authoritative active route |
| LATCH-V3-008 | Same-edge legacy collision and MagiK return | Ownership, sequence, epoch, and `LFB_*` stay consistent |
| LATCH-V3-009/010 | Reject and every counter/sequence wrap | Soak has no unexpected reject/drop delta |
| LATCH-V3-011 | Rust v2/v3 negotiation and CRC/no-downgrade regressions | v2 rollback works; old-runtime/v3 pairing rejects safely |
| LATCH-V3-012 | Direct RTL and production-bridge simulation | Stock comparison, lifecycle, and rollback |

## Candidate and custom-delta gates

A candidate is eligible only when all of the following are retained:

1. Merged MagiK commit, pinned Menu commit, FPGA component ID, protocol version
   and hash, patch/RTL/bridge hashes, Quartus 17.0 identity/seed, RBF hash,
   immutable workflow URL, and every report hash.
2. Deterministic contract generation, direct RTL simulation, production-bridge
   integration simulation, warning-clean lint, complete reachable line/branch
   coverage, and explicit functional coverpoints.
3. A stock-versus-patched Quartus comparison with identical source, toolchain,
   settings, and seed. The patch adds no warning, inferred latch, unconstrained
   endpoint, negative slack, nonzero TNS, or unreviewed CDC finding.
4. Hardware evidence bound to the candidate: 20 cold starts, 100 launcher
   first-use restarts, 100 Main-mediated RBF reload/returns, 50 core
   handoff/returns, at least 10,000 posts under load, deliberate replacement
   and corruption, HDMI 720p/1080p, CRT 240p/288p/480p/576p, a two-hour primary
   soak, and a second-unit/display matrix.
5. Zero unexpected reject/drop increments, CRC or malformed-status errors,
   persistent latch episodes, crashes, or reboots. Physical sink observation
   remains mandatory; framebuffer content alone is insufficient.
6. New runtime with v2 and v3 RBFs, old runtime with v2, safe rejection of the
   interrupted old-runtime/v3 pairing, v3-to-v2 rollback, update_all overwrite,
   and fresh installation.
7. Independent Rust and FPGA review of the final diff, component identity,
   workflow triggers, updater safety, rollback, and documentation.

## Historical artifacts

Protocol-v2 RBFs and their evidence are retained only for rollback validation.
They require the verifier's explicit `--historical-v2` mode and cannot satisfy
a new build or qualification gate. The superseded candidate disposition is in
[FPGA latch release signoff](fpga-latch-release-signoff.md). A candidate hash
listed there is not approval for protocol v3.

The implementation PR ends after its merge and successful checks. The
**Build MiSTer MagiK Platform** workflow is dispatched separately on `main`
with `publish=false`; promotion and attended `scripts/agent release qualify`
remain separately authorized gates.
