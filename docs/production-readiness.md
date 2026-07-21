# Production readiness

A release candidate is ready only when its exact commit has passed
`scripts/agent verify`, platform CI qualification where selected, transactional
delivery, and an attended release qualification.

```text
scripts/agent commit -m "Describe the release candidate"
scripts/agent deliver
scripts/agent release qualify
```

`release qualify` has no tiers, skips, or substring filters. Its fixed state
chart covers runtime health, catalog validity, input and game handoff/return,
display evidence, recovery capability, and restoration. It refuses non-terminal
execution and requires the operator to confirm continuous attendance plus a
non-network recovery path before creating its volatile session token.

Automated fake-device qualification and real-device attended evidence are
recorded separately. A skipped physical capability is not a pass. Destructive
recovery never runs unattended, and no release command pushes Git state.

Rollback is verified with the typed `mister mode stock` operator command.
