# Catalog build starvation reproduction

Date: 2026-07-28

## Customer diagnostic evidence

The supplied diagnostic archive is private input and is not stored in Git.
Its relevant sequence was:

- cold catalog process `1186` began at 03:54:07 EEST;
- the first-visible Arcade catalog completed in about 13 seconds with 906
  games;
- durable build `4a2-18c64dff362d4378` planned 170 full-scan targets;
- the last scanner event was `walk_targets`, with no target completion,
  journal growth, manifest publication, crash, reboot, or memory exhaustion
  during the following 75 minutes 47 seconds;
- a later launcher process found no valid catalog manifest.

The diagnostics could not distinguish a scanner syscall stall from a
cooperative-permission stall because they did not include the current target,
target heartbeats, durable journal counters, or the background gate blockers.

## Controlled device reproduction

Installed diagnostic runtime revision:
`686d3d0dd17eb2e142e3f0b40689b372478503b4`

The typed `catalog-lifecycle` benchmark restarted the real launcher with every
catalog path redirected below
`/tmp/mister-magik/catalog-lifecycle-benchmark`. Its deterministic input script
remained active longer than the fixed completion deadline, reproducing a
launcher that never supplies the old two-second interaction-idle window.

Observed result:

- first-visible Arcade catalog: 18,623 ms, 917 games;
- full V3 manifest: absent after 1,200 seconds;
- terminal inspection: `valid=0`, `no valid manifest slot`;
- device reboot: none;
- cleanup: isolated fixture removed and the ordinary Home launcher restored
  successfully as PID 5460.

This reproduces the reported symptom on the installed library: Arcade becomes
usable promptly, the launcher remains alive, but the authoritative all-system
catalog never publishes while interaction remains active.

The initial harness collected its detailed diagnostic directory only on the
success path. Commit `effadaf1` corrects that evidence-preservation gap for
future failures. The timeout, first-visible timing, terminal manifest
inspection, successful cleanup, and direct TV observation remain sufficient to
establish the starvation behavior.

## Scheduling conclusion

The cold-build parent retained the foreground all-core role after publishing
first-visible Arcade, while its full walker entered the CPU0 background role.
That walker was controlled by an input-derived global permission gate.
Scripted input, controller changes, navigation motion, and preview activity
could continuously reset the gate, so background checkpoints made no forward
progress.

The required correction is to transition the whole post-reveal catalog
pipeline to the background CPU0 role and keep it continuously permitted.
Controller, navigation, media, and preview activity must not suspend catalog
work. Foreground bootstrap work remains all-core until first-visible
publication; the no-first-visible failure path remains foreground.
