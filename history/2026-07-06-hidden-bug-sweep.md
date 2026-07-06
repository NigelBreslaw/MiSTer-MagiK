# Hidden bug sweep - 2026-07-06

This sweep used parallel read-only agents across input, framebuffer/preview,
catalog, launch handoff, desktop, host tools, and private media tooling. Each
fix landed with a regression test that failed before the fix and passed after.

## Verified fixes

1. Framebuffer stream strided rects now reject rows that overrun the source
   stride.
2. Desktop framebuffer capture parsing rejects unknown encodings instead of
   treating them as raw bytes.
3. Desktop frame-profile TSV parsing rejects rows with missing declared cells.
4. Desktop frame-profile dominant phase is `unknown` when no phase has time.
5. Setup live hints treat the idle marker as idle input.
6. Catalog navigation projection preserves per-game preview archive paths.
7. MGL metadata parsing reads `<file>payload</file>` text payloads.
8. Covered-payload matching normalizes `.` and `..` path components.
9. `scripts/mister run` validation rejects direct arcade scene invocations even
   when hidden behind simple shell variable indirection.
10. Deploy transaction validation rejects remote paths containing `.` or `..`.
11. Profile-count parsing skips option values such as `--timeout 30`.
12. Generic button 13 maps to Capture.
13. FS fault reset cleanup removes the rebuild-on-next-boot marker.
14. Console screenshot staging rejects input and output being the same
   directory before deleting output files.
15. Neo Geo screenshot staging rejects input and output being the same
   directory before deleting output files.
16. MAME screenshot staging rejects input and output being the same directory
   before deleting output files.
17. MagiK launch handoff accepts the first fresh status when no previous
   baseline exists.
18. MagiK launch timeout cleanup clears the input-policy marker.
19. Desktop SD browser path normalization collapses parent components.
20. Raw565 preview decoding rejects zero dimensions.
21. Preview screen rects clamp to small framebuffer dimensions.
22. Preview cache jobs reject case-folded duplicate stems.
23. Private raw565 pack parsing rejects zero dimensions.
24. Private raw565 pack parsing rejects odd byte strides.
25. Private raw565 pack parsing rejects decoded payloads too large for the
   archive format.

## Follow-up candidate

The generic controller profile still conflates stick, d-pad axes, and hat
sources into one set of booleans. A neutral event from one source can clear a
different source that is still held. Fixing this cleanly needs per-source
direction state rather than another one-off mapping tweak.
