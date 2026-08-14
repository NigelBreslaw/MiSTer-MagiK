# Production readiness

A release candidate is ready only when its exact commit has passed pre-push and
CI assurance, platform qualification where selected, transactional delivery,
and an attended release qualification.

```text
git add -- PATH...
git commit -m "Describe the release candidate"
scripts/agent deliver
scripts/agent release qualify
```

`release qualify` has no tiers, skips, or substring filters. Its fixed state
chart covers runtime health, catalog validity, input and game handoff/return,
display evidence, recovery capability, and restoration. It refuses non-terminal
execution and requires the operator to confirm continuous attendance plus a
non-network recovery path before creating its volatile session token.
It also requires the exact-candidate physical return-video aggregate described
in `docs/return-video-qualification.md` before it creates that token or performs
any device mutation.

When Main or the MagiK Menu RBF changes, the same candidate must first satisfy
`docs/bootstrap-black-qualification.md`; then the full latch/platform release
gate is rerun. The bootstrap evidence set is additive and never substitutes for
the six-hour latch stress or display matrix.

Automated fake-device qualification and real-device attended evidence are
recorded separately. A skipped physical capability is not a pass. Destructive
release fault injection never runs unattended; this is distinct from the
single bounded raw recovery reboot owned by `scripts/agent diagnose`. No release
command pushes Git state.

Rollback is verified with `scripts/agent device mode set stock --attended`.
