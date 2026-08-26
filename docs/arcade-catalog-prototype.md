# Arcade-only catalog prototype

The Arcade-only catalog prototype is an isolated production-code experiment. It
does not replace Catalog V3, import its scanner, publish into its registry, or
change the launcher. Its purpose is to establish the minimum cold-build cost
when Update_All metadata is treated as an ahead-of-time source of truth.

The executable is `arcade-catalog-prototype` in the `mister-magik-catalog`
package. The typed ARM build and hardware benchmark are:

```bash
scripts/agent build arcade-catalog-prototype-device
scripts/agent benchmark arcade-catalog-prototype-cold
```

The benchmark is the only authority for timing. Its normal indexed build,
filtered fast build, and filtered full-walk recovery each use a separate
supervised Linux reboot. Every arm waits for launcher health, suspends through
Main's acknowledged command, removes the active output, syncs, drops the Linux
page, directory-entry, and inode caches, and creates a new active catalog.
Cleanup resumes and health-checks the Dev launcher and removes the isolated
benchmark root.

The recovery test copies the installed Update_All index, removes one game that
is proved to have an installed MRA, ROM archive, and RBF, and compiles a
separate filtered base. Fast mode must omit that game. `--full-walk` must parse
the now-unknown MRA and restore it. Production Update_All and Catalog V3 files
are never modified.

The runner installs the repository's attended-operation signal guard so an
interrupt is converted into bounded cleanup rather than terminating after Main
has been suspended. The focused binary is uploaded through a temporary path,
matched to the exact local SHA-256, atomically published, and rehashed after
every reboot. The precompiled source base is likewise rehashed after every
reboot. Each active output is decoded with `inspect`, checked against its build
report, remotely hashed, downloaded, and rehashed. The retained summary is
bound to the clean source commit and must prove that the production and Dev
Catalog V3 registry manifests did not change during the isolated run.

## Data flow

The prototype separates immutable source knowledge from the card-specific
active catalog:

```text
Update_All Arcade index
        |
        | compile-base (ahead of boot/update time)
        v
checksummed source base (3,069 candidates on the measured corpus)
        |
        | build-active (fresh after reboot)
        +---- shallow MAME/HBMAME ZIP-name inventory
        +---- directory-batched tests of likely installed MRA names
        +---- deterministic variant/family selection
        v
atomic active catalog (1,181 records on the measured corpus)
```

`compile-base` independently validates the Update_All LZ4 size prefix, format,
payload checksum, source order, path safety, and row invariants. It compiles
normalized identity, family, metadata, ROM namespace/set name, expected size,
and variant score into a fixed-record/string-table binary. The measured source
base is 703,617 bytes.

`build-active` reads ZIP file names only from the four Main-compatible MAME and
HBMAME locations. It does not open ROM archives. A missing ROM eliminates its
candidate before MRA discovery. An ambiguous Update_All ROM requirement fails
closed in the fast mode and does not cause a card read. Candidates that remain
are grouped by parent directory. Each relevant exFAT directory is enumerated
once and names are matched case-insensitively in memory, avoiding one metadata
lookup per expected MRA. The default route accepts Update_All metadata when the
indexed path is present and a regular file; `--verify-index-size` adds file-size
validation.

The active output is a checksummed, versioned, fixed-record/string-table binary.
It retains every playable variant and marks one deterministic preferred record
per family. Publication writes a new sibling temporary file, syncs it, renames
it atomically, then syncs the parent directory.

## Commands

Compile immutable Update_All knowledge outside the boot-critical path:

```bash
arcade-catalog-prototype compile-base \
  --updater-index arcade-updater-index-v1.lz4b \
  --output arcade-source-base.bin
```

Create a fresh active Arcade catalog from that base:

```bash
arcade-catalog-prototype build-active \
  --base arcade-source-base.bin \
  --output arcade-active.bin
```

`build` performs both phases and is retained as the stricter end-to-end control.
`inspect` validates either binary and reports its source checksum and counts.

Discovery is unconditionally single-worker. Five reboot-cold comparisons found
the two-worker implementation slower in every production-shaped active build,
so it is retained only in the dated evidence and not in the executable.
`--full-walk` is the recovery/completeness route for custom MRAs absent from
Update_All; it is intentionally not the fast default.

## Trust and scope

Fast mode is deliberately asymmetric:

- Update_All supplies likely MRA paths and metadata.
- The current card proves which indexed paths and ROM archives are present.
- Ambiguous source rows fail closed.
- Unindexed custom MRAs require `--full-walk`.
- No warm-cache timing is accepted.

Before promotion, three additional authority gaps must be closed. A modified
MRA at an expected path can inherit Update_All metadata unless size/hash or an
installer receipt proves it is the indexed file. The active binary does not yet
carry a ROM/MRA inventory fingerprint or generation proving that a retained
file matches the current card. Publication is one atomic replacement rather
than Catalog V3's alternating valid-generation recovery contract.

This output is not schema-compatible with Catalog V3 SQLite, NavPack, search,
scanner cache, resumability, or registry publication. Promotion therefore needs
an adapter or a new launcher reader plus parity qualification for launch paths,
family/variant policy, metadata, search/navigation needs, interrupted
publication, custom MRAs, and Update_All version skew. The prototype proves the
discovery and compact-build opportunity; it does not by itself authorize a
production catalog migration.

The dated measurements and legacy comparison are recorded in
[`history/2026-08-26-arcade-catalog-prototype-performance.md`](../history/2026-08-26-arcade-catalog-prototype-performance.md).
