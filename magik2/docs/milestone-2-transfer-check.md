# Milestone 2: transfer comparison stopped for user input

The new worktree is on `nigel/mini-magik-shared`, based on merged main. The
two-attempt acceptance change is preserved. The old worktree and old local tooling branches were removed as requested.
The remote tooling branch was already absent. Historical acceptance
artifacts are retained outside Git in the primary checkout's
`outputs/magik2-milestone1-artifacts/build` directory.

## First comparison — 5 September 2026

Both paths transferred the same installed development app binary: 31,763,900
bytes, SHA-256 `eb31a49e221246a23c70ef55a1cfe1d368534670522c0a9567782eed1ae8680d`.
The binary was downloaded once as untimed preparation. Neither transfer started
it or replaced an installed app, Main or platform manifest.

| System | Receive, verify and save | Sustained MB/s | Sustained Mb/s | Host request to acknowledgement |
|---|---:|---:|---:|---:|
| Legacy | 6,021 ms | 5.276 | 42.204 | 6,034 ms |
| New | 8,430 ms | 3.768 | 30.144 | 8,504 ms |

Both write to the same SD filesystem and include payload verification, file
synchronization, rename and parent-directory synchronization. Device-reported
receive/save time is the comparison metric; host elapsed time is separate.
Both commands cleaned their staging files; the existing probe remained running.

The new result is approximately **28.6% lower throughput**. This is one pair,
not enough to establish a repeatable regression, but enough to trigger the
user's instruction to stop on a performance problem. One remaining attempt per
system was **not run**. No optimization or further application integration was
attempted after this result.

## Implemented so far

- `scripts/agent device transfer-check --artifact PATH --attended` measures one
  legacy native upload using its existing staging lock and save path. Optional
  `--fetch-installed` first retrieves the installed development binary into a
  new local file. No build or download time enters the measurement.
- `scripts/magik2 transfer-check --artifact PATH` measures one native 2.0 upload
  to a fixed disposable destination. It does not start the payload or retry a
  failed measurement. Missing command support installs automatically, untimed.
- The 2.0 publisher synchronizes the parent directory, matching the legacy save
  acknowledgement. Results include bytes, hash, saved throughput and host time.
- Focused host tests and the native staged-upload test passed; the legacy CLI
  passed focused Cargo check and Clippy.

Raw comparison results remain in `build/transfer-comparison/result.json` and
native diagnostic bundles in `build/magik2-results/`; binaries are not committed.

## Suggested next step — requires user direction

Finish the remaining **one new and one legacy transfer**, without changing any
transfer code. If the difference repeats, propose a separate bounded diagnostic
that distinguishes receive/hash time from write/synchronization time. Do not
change buffer sizes, hashes, compiler flags or synchronization, and do not add
further rounds, without the user's decision.

The remaining milestone work is deliberately pending: Mini-MagiK naming and
application selection; shared application-side observation/measurement/profile
support; real-app development-copy integration; and the small per-app validation
batch. See the approved milestone plan in the task for those requirements.
