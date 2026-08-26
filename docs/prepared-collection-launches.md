# Prepared collection launches

MagiK can reuse launch artifacts supplied by collections that are already installed on a MiSTer. It does not download, install, extract, update, or recommend collection payloads.

## Supported adapters

- **AmigaVision / MegaAGS**: discovers complete modern or legacy installations, indexes their game and demo listings, writes the exact selected title to the installation-local `shared/ags_boot`, and launches the real Amiga MGL.
- **0MHz**: recognizes 319 v0.04 launchers from a checked-in per-game release manifest and validates their referenced AO486 payload names with batched directory reads. Unknown, size-mismatched, and custom MGLs use the existing parser and validation fallback. Runtime launch validates the original MGL again and gives it to Main unchanged.
- **Neon68K**: discovers per-game MGL files under the current `_Computer/_X68000 Games` root and the legacy `_Computer/X68000 Games` root, validates X68000, setname, and HDF references, and preserves compatibility-folder metadata. Either exact launcher root may be a symlink; traversal follows only that root link, ignores every nested symlink, and prunes the real `_Genre` alias tree so MiSTer collection views cannot duplicate discoveries or expand unrelated scans.
- **OneLoad64**: recognizes signed collection directory layouts, indexes primary and MultiLoad64 CRTs, excludes dump/alternative/extras trees, and loads CRTs through C64 file index 1. Its reusable helper covers and prunes only the recognized OneLoad64 install directory; other C64 directories remain on the normal scanner.

Raw and generic entries remain visible. When prepared and generic rows have the same title and system, the prepared row sorts first.

## Catalog diagnostics

Prepared provenance is stored in `prepared_launch_rows` and exposed through `prepared_launches` and `launch_provenance`. Rejected collection candidates are recorded in `prepared_launch_diagnostic_rows` and exposed through `prepared_launch_diagnostics`.

When the Neon68K `boot3.vhd` payload signature exists but the launcher root is missing, broken, or unreadable, `catalog_audit` records the stable reason `neon68k-launcher-root-missing-or-unreadable`. A valid real directory or root symlink clears that diagnostic; invalid individual MGL files retain their per-launch diagnostics.

Examples:

```sql
SELECT collection_id, count(*)
FROM prepared_launches
GROUP BY collection_id;

SELECT title, collection_id, status, reason
FROM prepared_launch_diagnostics
ORDER BY collection_id, title;
```

The catalog stamp includes the adapter version and metadata fingerprints for relevant nested MGL, CRT, HDF, and listing files. Runtime preparation validates collection artifacts again before Main handoff so a stale catalog fails safely.

## Prepared bundle helpers

The builder feature now contains a standalone exact-release helper model. A
helper carries precomputed catalog entries, cryptographic receipts for small
metadata files, size receipts for large owned payloads, and optional inventories
of launchable paths. An exact match can reuse the precomputed entries without
re-parsing the collection. Any changed or missing receipt, additional launchable
file, older layout, partial install, or custom addition rejects the helper and
calls the normal collection scanner. The helper therefore accelerates only
content it can positively identify and cannot hide custom content.

AmigaVision and Neon68K use exact target helpers. OneLoad64 uses a subtree
helper so a personal C64 collection never becomes part of its cached output.
0MHz uses the checked-in per-game manifest rather than a target snapshot because
users can install any subset of the release. All feed the normal Catalog V3
projection and retain their existing adapters as the mismatch fallback.

## Acceptance

Host tests use synthetic payloads only. Device acceptance may launch collections only when they were independently installed by the device owner. It must not download content, reboot the MiSTer, or alter persistent launcher environment files.
