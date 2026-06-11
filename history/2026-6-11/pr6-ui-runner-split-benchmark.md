# PR6 ui_runner split benchmark

Parent commit: `b0e4274` (`Refresh frontend production wording`).
After commit: PR6 worktree, final commit containing this note.
Device: MiSTer at default `scripts/mister` target.
Scenario: arcade held-scroll velocity profile, 30 seconds, framebuffer format 565.

Commands:

```bash
scripts/profile-arcade-scroll.sh 30 PR6-BEFORE-20260611 --deploy-fast
scripts/profile-arcade-scroll.sh 30 PR6-AFTER-20260611 --deploy-fast
scripts/profile-arcade-scroll.sh 30 PR6-AFTER2-20260611 --skip-build
```

Artifacts:

- `build/arcade-scroll-profiles/PR6-BEFORE-20260611-arcade-scroll.tsv`
- `build/arcade-scroll-profiles/PR6-BEFORE-20260611-arcade-scroll.log`
- `build/arcade-scroll-profiles/PR6-AFTER-20260611-arcade-scroll.tsv`
- `build/arcade-scroll-profiles/PR6-AFTER-20260611-arcade-scroll.log`
- `build/arcade-scroll-profiles/PR6-AFTER2-20260611-arcade-scroll.tsv`
- `build/arcade-scroll-profiles/PR6-AFTER2-20260611-arcade-scroll.log`

Summary:

| label | frames | fps | wall p95 | wall max | custom_draw p95 | fb_present p95 | cached_present p95 | overlay_present p95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `PR6-BEFORE-20260611` | 1800 | 60.0 | 16497us | 19502us | 9276us | 3373us | 524us | 2867us |
| `PR6-AFTER-20260611` | 1799 | 60.0 | 16535us | 18495us | 10750us | 3391us | 571us | 2881us |
| `PR6-AFTER2-20260611` | 1800 | 60.0 | 16483us | 18010us | 10688us | 3378us | 514us | 2872us |

Conclusion: no frame-pacing regression from the module split. The first after
run showed higher `custom_draw` p95, so it was rerun with the deployed binary.
The confirmation run kept 60 fps, slightly improved wall p95 versus baseline,
and matched framebuffer present timings within normal run-to-run noise.
