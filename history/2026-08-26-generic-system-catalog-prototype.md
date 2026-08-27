# Generic filesystem catalog prototype

Date: 2026-08-26

Branch: `nigel/arcade-catalog-prototype`

## Design

The independent catalog now has a generic path for user-managed ROM folders.
It does not read the old catalog, require a known collection release, or assume
a fixed directory tree below the system folder. The prototype recursively
catalogs ZX Spectrum, SNES, Neo Geo, and Sega Saturn using only the installed
core launch contract:

- valid direct-file and ZIP-member extensions;
- core selector and mount operation;
- ignored BIOS, support, and cue-track files;
- arbitrary user-created nesting below each game directory.

Direct files are never opened, hashed, or statted. ZIP payloads are not
decompressed; only the central directory is read. A focused core-directory
probe replaces broad profile activation, so unrelated storage is never walked.
Neo Geo ROM-set ZIPs remain one launchable game. Saturn BIN/IMG cue tracks do
not become duplicate games.

## Reboot-cold result

The retained run started after a supervised reboot with cold memory and
filesystem caches. It used Postcard-mmap input and search-only SQLite plus
NavPack output.

| System | Files visited | Games | Cold discovery | Artifact build | SQLite | NavPack |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Neo Geo | 823 | 275 | 0.523 s | 0.117 s | 200.0 KB | 78.5 KB |
| Sega Saturn | 1,592 | 368 | 0.601 s | 0.129 s | 216.0 KB | 88.0 KB |
| SNES | 5,280 | 2,928 | 1.524 s | 0.843 s | 1,420.0 KB | 715.7 KB |
| ZX Spectrum | 863 | 860 | 0.332 s | 0.362 s | 416.0 KB | 198.2 KB |
| **Four-system discovery** | **8,558** | **4,431** | **3.211 s** | — | — | — |

Three SNES files with `.zip` names had unreadable ZIP central directories and
were safely skipped; all direct SNES payloads were retained. There were no
filesystem read errors and no archive errors in the other systems.

Removing broad profile activation reduced discovery from 42.304 seconds to
11.225 seconds. Removing unnecessary per-ROM metadata reads then reduced it to
3.211 seconds: 3.50x faster than the focused metadata-reading version and
13.17x faster than the initial generic prototype.

The final catalog contained nine systems, 22,411 visible games, and 98 retained
C64 variants. Exact verification found zero changed rows. The real Dev UI
loaded all nine systems with catalog refresh disabled. Publication of all nine
systems took 9.378 seconds, or 10.154 seconds including snapshot access and
command overhead; 6.110 seconds of that remained C64 artifact work.

## Evidence

- Final cold report:
  `build/agent-benchmarks/generic-system-prototype/cbd22a109-cold.json`
- Focused scanner before direct-file metadata removal:
  `build/agent-benchmarks/generic-system-prototype/9b0d8bf1b-cold.json`

The isolated catalog did not alter the production registry. Device reboot and
fault arming state was clear after the run. No commit was pushed.

## One-sample old-builder comparison

One independent reboot-cold old-builder sample was run for each generic
system. `Old builder` is the authoritative-catalog-prepared phase. `New total`
adds the retained generic discovery and per-system search/NavPack artifact
build phases.

| System | Old games | New games | Old builder | New total | Ratio |
| --- | ---: | ---: | ---: | ---: | ---: |
| Neo Geo | 173 | 275 | 11.577 s | 0.640 s | 18.08x |
| Sega Saturn | 348 | 368 | 11.029 s | 0.729 s | 15.12x |
| SNES | 1,805 | 2,928 | 10.567 s | 2.367 s | 4.46x |
| ZX Spectrum | 859 | 860 | 10.602 s | 0.694 s | 15.27x |
| **Summed phases** | **3,185** | **4,431** | **43.774 s** | **4.431 s** | **9.88x** |

The differing game counts are important: this is an observed end-to-end
comparison, not an equal-row microbenchmark. The generic prototype completed
substantially more discovery work, particularly for custom SNES and Neo Geo
content. The old matrix used isolated roots and did not alter the production
registry.

Old-builder evidence:
`build/agent-benchmarks/generic-system-old-cold/2261e27e0.json`.
