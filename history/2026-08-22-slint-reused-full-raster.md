# Reused-buffer full raster experiment — 2026-08-22

## Authority

- Installed experiment revision: `58286b11821fb460daee73e72337173fe37a8dbd`
- Main revision: `639d3694e1b93660020e9587cd0fe27f0170ce4c`
- Control: `build/agent-benchmarks/settled-composition/1787372829/summary.json`
- Candidate: `build/agent-benchmarks/settled-composition-reused-cache/1787372853/summary.json`
- Performance authority: unprofiled installed Dev runtime

## Result

The candidate retained `RepaintBufferType::ReusedBuffer`, marked the full
logical region through Slint 1.17.1's pinned `RendererSealed` dirty-region seam,
and rendered through the ordinary reused-buffer path.

| Metric | New-buffer control | Reused-buffer candidate | Delta |
| --- | ---: | ---: | ---: |
| Destination raster | 12,658us | 12,055us | -603us (-4.8%) |
| Following-frame raster | 0us | 0us | 0us |
| Two-frame raster | 12,658us | 12,055us | -603us (-4.8%) |
| Two-frame copied bytes | 0 | 0 | 0 |

Both arms produced the identical authoritative terminal Settings PNG hash:
`42e7ef05f7510300246df562e67c54c9de481cf6abc70651db0917e40eabd58c`.
Both also passed with zero repeated vblanks, latch drops, ownership losses,
sequence gaps, and phase outliers.

## Attribution and disposition

The next frame was already zero-raster in the control. The candidate therefore
removed only repaint-policy/cache-clear overhead; the full 1280x720 destination
raster remained dominant. The 4.8% improvement fails the required 20% gate and
does not reach the 16% threshold that would permit a second recovery.

The bounded recovery used the version-pinned `i-slint-core` logical geometry
adapter after the public Slint-root geometry import failed the ARM build. It
delivered and proved correctness, but it did not change the performance
ceiling. Do not mutate private Slint cache storage: revert the experiment and
retain the existing new-buffer production path.
