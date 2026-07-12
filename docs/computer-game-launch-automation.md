# Computer game launch automation research and plan

Status: research proposal, 2026-07-11

## Executive finding

MagiK can become dramatically more useful for computer software, but the right
goal is not “automate the MiSTer OSD.” The scalable goal is:

> Turn every catalog row into a truthful, declarative launch contract that
> loads a core, attaches the right media, performs any required reset/input
> sequence, and clearly reports when the remaining step cannot be automated.

There are three complementary ways to do this:

1. **Use Main_MiSTer's direct core APIs.** Load/mount media by slot, reset the
   core, and inject bounded keyboard/joystick events without navigating the OSD.
2. **Reuse prepared game artifacts.** AmigaVision, 0MHz, OneLoad64, and
   Neon68K make computers act like consoles by preparing WHDLoad/HDF, VHD, CRT,
   or HDF artifacts that boot one game. MagiK should recognize and integrate
   these rather than recreate their curation.
3. **Use game-specific recipes only where necessary.** Timed input is useful,
   but inherently less reliable than bootable media or a direct-load format.

“Every system” is a reasonable coverage-program goal. “Every arbitrary disk
image starts perfectly” is not technically honest: some software needs a
particular machine model, writable media, several disks, an installation,
copy-protection handling, or human answers to prompts. MagiK can still cover
those systems by distinguishing verified one-click launches from best-effort
automation and “media mounted, user step required.”

## What the current implementation does

MagiK already has most of the handoff plumbing:

- Catalog rows can resolve to `StructuredLaunchPlan`.
- The launcher sends `mister_magik_launch_plan_v1` to the maintained
  `MiSTer_MagiK` fork.
- Main resolves the core, loads it, and seeds an MGL action.
- The current structured plan contains one payload path, one mount kind, one
  mount index, and one delay.

The important mismatch is that stock MGL supports more than this. An MGL can
contain multiple file actions, a `setname`, and reset actions. Main's MGL
executor currently stores up to six timed file/reset actions. The structured
MagiK plan flattens that capability down to one file action.

The maintained Main fork also already has the low-level functions needed for a
future input sequencer:

- `user_io_kbd(...)`
- `user_io_digital_joystick(...)`
- `user_io_l_analog_joystick(...)`
- `user_io_r_analog_joystick(...)`
- `user_io_file_mount(...)`
- `user_io_file_tx(...)`
- core reset/status operations

These should be called by Main after handoff. A surviving MagiK child, shell
script, raw `/dev/uinput` writer, or OSD-driving robot would add unnecessary
failure modes.

## Evidence from the live device

The device catalog audit reports:

- 72 installed `_Computer` cores in the audit.
- 40 have a catalog profile.
- 32 have no catalog profile.
- 48,823 current computer launch targets in the sampled configured cores.
- Nearly all of those targets use the same generic `virtual-mgl/load-file`
  strategy even though the files have very different semantics.

Large examples are:

| Core | Current rows | Current generic action | Why that is unsafe |
|---|---:|---|---|
| C64 | 32,751 | `load-file` | CRT/PRG direct loading is not the same as D64/G64/D81 disk mounting and booting. |
| BBC Micro | 4,734 | `load-file` | SSD/DSD/UEF need media and boot semantics, not one universal file transfer. |
| PC-88 | 3,831 | `load-file` | ROMs, disks, archives, and boot configurations are not interchangeable. |
| Oric | 982 | `load-file` | Tape/disk boot behavior is distinct from direct memory loading. |
| C16 | 959 | `load-file` | PRG, TAP, and disk images need different actions. |
| ZX Spectrum | 860 | `load-file` | Snapshots can load directly; TAP/TZX and TRD require different boot flows. |
| Acorn Electron | 817 | `load-file` | UEF and SSD are different media classes. |
| Atari ST | 510 | `load-file` | ST floppy images normally need mounting and reset/boot. |
| X68000 | 302 | `load-file` | D88 floppy and HDF/VHD hard disks need slot-specific mounting and boot. |

This is both a launch problem and a catalog-truth problem. Discovering a file
that a core can select does not prove that the file is a game or that selecting
it starts that game.

## What existing projects teach us

### MGL

The [official MGL documentation](https://mister-devel.github.io/MkDocs_MiSTer/advanced/mgl/)
explicitly says computer cores generally work poorly with MGL unless the loaded
software has an auto-boot mechanism. It also documents multiple file actions,
reset actions, `setname`, and source-derived `F`/`S` slot indices.

The lesson is that MGL is a useful transport and action envelope, but not a
universal computer-game loader by itself.

### AmigaVision

[AmigaVision](https://github.com/amigavision/AmigaVision) builds an Amiga HDF
around WHDLoad/custom installs and a guest-side launcher. It solves disk swaps,
machine configuration, and per-game quirks in the environment that understands
the Amiga, then presents a console-like selection experience.

The lesson is to put complex boot knowledge in prepared media or a guest
launcher when that is more deterministic than host-side keystrokes.

### 0MHz DOS Collection

[0MHz](https://github.com/0mhz-net/0mhz-collection) uses one configured VHD per
game and very small MGL files. A simple game mounts one VHD and resets AO486. A
CD game can mount both its VHD and CHD before reset.

The lesson is that full MGL action parity immediately unlocks existing,
well-tested per-game DOS launches. Raw AO486 VHD/CHD/ISO files should remain
attached media unless a recipe or collection descriptor proves they boot one
specific game.

### OneLoad64

[OneLoad64](https://oneload64.github.io/) converts C64 tape/disk releases to
auto-starting CRT artifacts. The project reports more than 2,100 games and
retains loading screens/music where practical.

The lesson is that a direct-load cartridge is far more reliable than teaching a
generic launcher every C64 loader and copy-protection variation. MagiK should
prefer a recognized CRT variant when one is installed, while still supporting
best-effort D64 flows.

### Neon68K

[Neon68K](https://neon68k.com/2025.04.29) supplies individually launchable
X68000 HDF games, MGL launchers, and per-game setnames/configuration. Its public
release describes over 200 tested games.

The lesson is that per-game configuration is part of the launch contract, not
just the media path.

## Automation strategy hierarchy

Use the highest available tier for a particular installed artifact:

| Tier | Strategy | Reliability | Examples |
|---|---|---|---|
| A | Direct executable/cartridge/snapshot load | Highest | C64 CRT/PRG, Spectrum Z80/SNA/SZX, Atari XEX, cartridge ROMs |
| B | Prepared per-game boot media | Highest | 0MHz VHD, Neon68K HDF, AmigaVision/WHDLoad, bootable Apple II disk |
| C | Mount media plus reset/core hotkey | High | Atari 800 “Boot Disk,” Apple II disk boot, bootable ST/Amiga floppy |
| D | Mount media plus bounded input recipe | Medium | C64 `LOAD"*",8,1`/`RUN`, Spectrum tape loader, Amstrad `RUN"name` |
| E | Guest-side selector/bootstrap | High after installation | AmigaVision, a DOS/ MSX/Acorn hard-disk launcher |
| F | User-assisted launch | Honest fallback | Ambiguous disk directory, install disks, protection/manual prompts |

The catalog may show Tier F content, but it must not label it “one-click” or
silently pretend that a mount action equals a successful game launch.

## System-family assessment

### Commodore 8-bit: C64, C128, C16/Plus4, VIC-20, PET

- Prefer CRT/cartridge and direct PRG formats.
- Integrate OneLoad64 as a first-class collection.
- For disk images, parse the directory where possible and offer a recipe that
  mounts the disk, waits for BASIC, then uses a core hotkey or symbolic key
  sequence. C64 already exposes `Alt+Esc` for `LOAD"*"` followed by `RUN`.
- Keep TAP as a separate tape recipe with explicit kernal/tape requirements.
- Treat trainer/crack prompts as optional post-launch input helpers, not proof
  that the game reached gameplay.
- Preserve write protection and never mutate an archival image by default.

### Sinclair: ZX Spectrum, ZX81, ZX Next/TSConf

- Prefer snapshots for one-click starts.
- TAP/TZX can mount and enter the core's tape loader; the Spectrum core exposes
  `F10` to switch to 48K BASIC and issue `LOAD""`.
- TRD/SCL needs a distinct TR-DOS/GLUK recipe. Not all images autostart.
- Model selection (48K/128K/+3/Pentagon) belongs in the recipe compatibility
  data, not in filename heuristics alone.
- ZX Next and TSConf need dedicated capability research before catalog rows are
  promoted to playable.

### Amstrad CPC and PCW

- DSK/CDT/ROM are separate media roles.
- CPC DSK automation can parse the disk directory, mount, and type a curated or
  inferred `RUN"...` command.
- Ambiguous disks should offer candidates rather than guess permanently.
- Tape requires `|TAPE`, `RUN"`, and playback semantics.
- PCW needs its own boot/application model; do not inherit CPC rules by name.

### Atari 8-bit

- Add a dedicated profile for ATR/ATX/XEX/CAS/cartridge formats.
- Use Main's existing Atari-specific “Boot Disk” and “Boot Tape” operations;
  they already perform the required reset/Option/Start/space choreography.
- Prefer direct XEX loading when compatible.
- Record PAL/NTSC, BASIC enabled/disabled, drive timing, and write policy as
  recipe options or verified overrides.

### Apple II

- Most bootable floppy images can be mounted and booted automatically.
- HDD images need a reset or `PR#7`; floppy changes may need `PR#6`.
- DSK/DO/PO ambiguity is real and should be captured by verified overrides or
  hashes rather than filename extension alone.
- Integrate known guest collections such as Total Replay when present, while
  retaining direct bootable-disk support.

### Amiga / Minimig

- ADF can use multi-drive mount plus reset for genuinely bootable titles.
- Group multi-disk sets into one game with swap metadata.
- Prefer AmigaVision/WHDLoad/HDF for broad reliable coverage.
- Machine configuration (Kickstart, chipset, CPU, RAM, PAL/NTSC) and writable
  save behavior are part of the recipe.
- Do not treat every HDF/ISO as a primary game.

### Atari ST

- Mount boot floppy in drive A and reset for auto-booting titles.
- Group multi-disk sets and expose disk-swap mappings.
- TOS version, ST/STe mode, memory, and hard-disk configuration need verified
  compatibility data.
- A curated hard-disk/guest launcher can provide higher coverage than macros.

### DOS / AO486 / PC XT / PCjr / Tandy

- Make 0MHz launchers first-class and preserve all file/reset actions.
- Raw VHD/CHD/ISO/IMG media are not games unless backed by a recipe.
- Prefer one bootable image per game or a guest bootstrap with a selected game
  identifier over typing DOS commands after a generic boot.
- PC XT, PCjr, and Tandy need separate core capability/boot profiles rather
  than inheriting AO486 assumptions.

### X68000 and other Japanese computers (PC-88, MSX, SVI)

- Prefer Neon68K's per-game HDF/MGL/setname route for X68000.
- Direct floppy boot remains useful, but requires correct drive order, reset,
  and sometimes per-game model/configuration.
- For MSX, prefer cartridge/direct ROM and established guest launchers when
  present; disk/tape software needs system-specific recipes.
- PC-88 archive/ROM/disk rows need a special profile. A generic `load-file`
  action is not sufficient evidence.

### Acorn/BBC, Oric, SAM, QL, TRS-80, CoCo, and smaller computers

- Build each profile from the core's `CONF_STR`, Main special cases, and core
  README, separating direct executable, floppy, tape, cartridge, hard disk,
  firmware, and snapshot roles.
- Use mount/reset first where disks are normally bootable.
- Add a small disk-directory parser or curated command recipe only where it
  materially increases coverage.
- Until verified, expose “start system with media mounted” rather than a false
  one-click claim.

### Installed cores without profiles

The current device reports these computer-oriented cores among the unprofiled
set: AliceMC10, Apogee, Apple-I, BK0011M, ColecoAdam, EDSAC, Enterprise,
Galaksija, Homelab, IQ151, Jupiter, Laser310, Lynx48, MultiComp, ORAO,
Ondra_SPO186, PCXT, PCjr, PDP1, PMD85, RX78, SharpMZ, SordM5, Specialist,
Svi328, TK2000, TSConf, Tandy1000, TatungEinstein, UK101, VT52, and Vector-06C
(the exact list changes with installed cores).

Each must get an explicit registry decision: supported direct launch,
supported recipe launch, media-only fallback, support/firmware-only, or no
playable content contract. “Installed core exists” must never itself create
game rows.

## Proposed architecture

### 1. Core capability registry

Replace the current extension-centric generic manifest with a generated and
reviewed capability registry. One core can have several media capabilities.

Each capability should record:

- core identity and aliases;
- source location and source commit/fingerprint;
- accepted extensions;
- media role: cartridge, direct executable, snapshot, floppy, hard disk, tape,
  optical, firmware, auxiliary, or unknown;
- Main operation: load-file, mount-image, or a named core-specific operation;
- slot/index and supported multiplicity;
- whether reset is needed;
- whether the medium is normally bootable;
- writable/read-only policy;
- compatible machine/configuration modes;
- evidence strength and last verification result.

Generate candidates by parsing each core's `CONF_STR` (`F...`/`S...` entries),
then require review for the semantic role. `CONF_STR` proves that a picker can
send or mount a file; it does not prove that the file is a game or auto-starts.

### 2. Launch Recipe v2

Move from one payload tuple to a bounded ordered action list. A conceptual
shape is:

```json
{
  "schema": 2,
  "core": "_Computer/AO486",
  "setname": "Doom",
  "artifacts": [
    {"id": "system", "path": ".../doom.vhd", "role": "hard-disk"},
    {"id": "cd", "path": ".../doom.chd", "role": "optical"}
  ],
  "actions": [
    {"op": "mount", "artifact": "system", "slot": 2, "after_ms": 0},
    {"op": "mount", "artifact": "cd", "slot": 4, "after_ms": 0},
    {"op": "reset", "after_ms": 1000, "hold_ms": 100},
    {"op": "release_all_input"}
  ]
}
```

Initial operations:

- `load_file`
- `mount`
- `reset`
- `wait`
- `key_down`, `key_up`, `key_tap`, `key_chord`
- `joystick_press`, `joystick_release`
- `release_all_input`
- named, reviewed core operations such as Atari `boot_disk`

Avoid arbitrary shell commands, executable scripts, unbounded loops, and OSD
coordinates. Use symbolic Linux key names so Main's existing keyboard mapping
translates them to the active core.

Recipes need hard limits: action count, string/path size, per-wait duration,
total duration, allowed paths, allowed key holds, and a guaranteed all-input
release on completion, cancellation, core change, or failure.

### 3. Main-side launch executor

`MiSTer_MagiK` should own execution after loading the core:

1. Parse and validate the recipe before stopping the launcher.
2. Resolve the core and every artifact path before handoff.
3. Load the core.
4. Execute actions from Main's normal event/timer loop.
5. Emit structured events for every action and terminal outcome.
6. Cancel remaining automated input on real user input unless the recipe marks
   the current step non-interruptible for a short bounded interval.
7. Release all synthetic inputs on every exit path.

Do not keep `mister-magik-fb` alive to inject input. Main already owns the
core-facing APIs and remains alive during gameplay.

### 4. Artifact and game grouping

Add an artifact model separate from the game row:

- one game can own several disks, tapes, CDs, configs, and save overlays;
- several alternative artifacts can represent the same game;
- a preferred launch variant can be selected by reliability tier;
- M3U, collection manifests, directory structure, filename patterns, and
  hashes can contribute grouping evidence;
- ambiguous grouping remains visible in audit data rather than silently
  merging games.

For writable media, default to a per-game working copy or overlay where the
core supports it. Never write into a ZIP member or known archival source.

### 5. Recipe sources and precedence

Use explicit precedence:

1. User override/sidecar.
2. Installed collection descriptor (0MHz, AmigaVision, OneLoad64, Neon68K).
3. Hash-matched curated recipe.
4. System/profile rule proven safe for that media role.
5. Best-effort inferred recipe.
6. Media-only fallback.

Store provenance and confidence in SQLite and expose it in diagnostics. A
recipe update must invalidate the warm catalog stamp.

### 6. Collection adapters

Do not repackage copyrighted game data. Adapters should discover installed
collections and import only launch metadata:

- parse full MGL action lists and setnames;
- read project-provided game listings/manifests;
- point at already-installed artifacts;
- retain upstream titles, IDs, configurations, and known-issue markers;
- identify upstream project/version for support and reproducibility.

## Product behavior

Every row should have a launch badge/state:

- **Instant**: verified direct load or prepared boot artifact.
- **Automated**: verified multi-step recipe.
- **Best effort**: inferred recipe; user can cancel input automation.
- **Media ready**: core starts and media is mounted, with a concise next-step
  overlay before handoff.
- **Unsupported**: retained in audit, not presented as a playable game.

If a best-effort launch fails, offer actions such as “try alternate boot
command,” “change machine model,” “mark media-only,” and “save working recipe.”
That turns user discoveries into reusable metadata instead of repeated manual
work.

## Implementation phases

### Phase 0: stop creating misleading rows

1. Split “selectable payload” from “launchable game” in profile semantics.
2. Disable runtime-generated computer game rows unless the media role and boot
   strategy are known.
3. Keep suppressed files in `catalog_audit` with a precise reason.
4. Add launch confidence/tier to catalog projection and UI.
5. Correct existing special profiles before expanding coverage.

This phase may temporarily show fewer games, but every remaining click becomes
more trustworthy.

### Phase 1: full MGL parity in structured plans

1. Define Launch Recipe v2 serialization and size limits.
2. Carry ordered file/reset actions and `setname` through catalog, snapshot,
   launcher, FIFO command, Main parser, and Main executor.
3. Import real MGLs without flattening them.
4. Preserve v1 decoding during migration.
5. Prove with 0MHz one-disk and disk+CD recipes and a multi-disk core.

This is the highest-confidence first implementation slice because Main already
executes these action types.

### Phase 2: bounded input automation

1. Add symbolic key/chord and joystick actions to Main.
2. Add interrupt-on-human-input and release-all safety.
3. Record action start/end/failure events.
4. Add deterministic host tests using a fake core-facing adapter.
5. Pilot C64 disk boot, Spectrum TAP, and one Amstrad DSK.

Do not add arbitrary `type_text` first. Start with symbolic keys and core
hotkeys; add text expansion only after keyboard layout and quoting behavior are
specified.

### Phase 3: prepared-collection adapters

Implement adapters in this order:

1. 0MHz (already MGL-shaped; validates multi-action parity).
2. Neon68K (validates HDF plus setname/per-game config).
3. OneLoad64 (validates preferred artifact selection and duplicate collapse).
4. AmigaVision (extend the existing listing support into an explicit launch
   contract rather than a shared opaque row).

### Phase 4: high-value native system profiles

Recommended order based on installed row volume and attainable reliability:

1. C64 family.
2. ZX Spectrum.
3. Atari 8-bit.
4. Apple II.
5. Atari ST.
6. Amstrad CPC.
7. BBC Micro/Acorn Electron.
8. X68000 and PC-88 outside curated collections.
9. Oric, SAM, QL, TRS-80, CoCo, MSX, VIC-20/PET.

Each profile ships as a vertical slice: capabilities, grouping, recipes,
catalog audit rules, fixture tests, and device acceptance evidence.

### Phase 5: complete installed-core registry

For every installed computer core, record an explicit decision even when the
decision is “no safe automatic launch.” Add a CI audit that fails when a newly
installed/source-harvested core has no reviewed decision.

## Validation plan

### Host tests

- Capability-registry parser tests from real `CONF_STR` fixtures.
- Recipe validation, encoding, decoding, bounds, and backward compatibility.
- MGL import fixtures with multiple files, reset, hold, and setname.
- Artifact grouping and ambiguity tests.
- Catalog projection tests proving firmware/auxiliary/media-only files do not
  become playable rows.
- Main executor tests with fake time and recorded core/input operations.
- Property/fuzz tests that every failing recipe releases all keys/buttons.

### Device tests

For each supported system, maintain a small legal/private fixture set covering:

- direct load;
- bootable disk;
- multi-disk or disk+CD;
- required reset;
- required keyboard sequence;
- writable save media;
- invalid/missing artifact;
- user interruption during automation.

Main events can prove that the correct core and actions executed. They cannot
prove that an arbitrary guest game reached its title screen. Game-visible
acceptance therefore needs attended HDMI checks or the existing external HDMI
capture path. `/dev/fb0`/agent still captures are not proof of game-core HDMI
output.

No part of this work requires reboot-fault testing or persistent launcher
arming. Normal launch/device tests must not touch the destructive reset-fault
paths.

## Success metrics

Measure by artifact and by user outcome, not by raw catalog row count:

- percentage of displayed computer rows with a verified launch contract;
- launch handoff/action failure rate;
- attended sample rate that reaches a playable title/menu without OSD use;
- median user actions from MagiK selection to gameplay;
- number of media-only/unsupported rows incorrectly shown as playable;
- coverage per installed core and per media role;
- recipe regressions across Main/core updates.

A good first milestone is not “all 48,823 rows automated.” It is:

> Every displayed row is truthful; 0MHz/OneLoad64/Neon68K/AmigaVision launches
> are first-class; and C64, Spectrum, Atari 8-bit, Apple II, Atari ST, and
> Amstrad have at least one verified automatic path for each major media role.

That would already make MagiK vastly more useful while establishing the model
needed to work through every remaining system without accumulating brittle
special cases.

## Recommended first tracer bullet

Implement Launch Recipe v2 with only `mount`, `load_file`, `reset`, `wait`, and
`setname`, then import and run these three real cases:

1. 0MHz Doom: VHD mount, delayed reset.
2. 0MHz 7th Guest: VHD plus CHD mounts, delayed reset.
3. One Neon68K title: HDF mount, setname, reset.

In the same change, suppress raw AO486 attached-media rows that lack a recipe.
This slice proves the schema, catalog, snapshot, launcher, Main handoff,
multi-action executor, collection provenance, and truthful fallback without
introducing keyboard automation yet.
