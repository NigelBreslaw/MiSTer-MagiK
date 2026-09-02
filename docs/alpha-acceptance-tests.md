# Local Alpha Device Tests

Physical MiSTer testing is an attended local operation. GitHub builds and
publishes immutable versioned payloads selected by the `alpha` feed, but it does not own or schedule access to
the developer's MiSTer.

The local journey uses the published alpha assets, the official MiSTer
Downloader, the real production UI, a real core launch and return, RGB565
checkpoints, and the fixed `USB Video` capture input.

## Run the journey

Create fresh local directories, resolve the current alpha version, download its immutable assets, and
run the typed workflow:

```bash
candidate_dir="$(mktemp -d)"
channel_dir="$(mktemp -d)"
evidence_dir="$(mktemp -d)/alpha-evidence"
gh release download alpha \
  --repo NigelBreslaw/MiSTer-MagiK \
  --pattern mister-magik-alpha-db.json --dir "$channel_dir"
version="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["release"]["version"])' "$channel_dir/mister-magik-alpha-db.json")"
gh release download "v$version" \
  --repo NigelBreslaw/MiSTer-MagiK \
  --dir "$candidate_dir"
scripts/agent alpha accept \
  --candidate "$candidate_dir" \
  --output "$evidence_dir"
```

The command verifies every downloaded checksum and packaged component, installs
the current `alpha` feed through Downloader, verifies the installed runtime against
the downloaded assets, and then exercises the physical device. By default the
MiSTer remains on the tested alpha.

Use `--restore-host-mode` when the original Main selection must be restored
after the journey. Use `--reuse-installed` to repeat the journey against the
same already-installed alpha without another Downloader run or reboot; it
cannot be combined with `--restore-host-mode`.

When the fixed `USB Video` input is unavailable, `--framebuffer-only` skips
physical HDMI captures while retaining the authoritative RGB565 checkpoints.
The receipt records this weaker evidence mode explicitly; it does not prove
HDMI sink visibility.

## What it tests

The bounded journey verifies:

- the real catalog becomes usable and completes successfully;
- Home, Arcade, filters, search, nested navigation, and Settings respond to
  production input handling;
- a safe real core launches through Main and returns to the same Arcade
  selection without rebooting;
- the restored launcher has the expected alpha version and source revision;
- framebuffer checkpoints and physical HDMI captures are nonempty and match
  their recorded hashes.

Failure after a core launch performs one bounded typed return attempt. Device
mutation remains serialized and cleanup follows the normal agent recovery
rules; the command never uses raw SSH or persistent reboot-fault state.

## Local evidence

A successful run writes `alpha-acceptance.json` below the chosen evidence
directory together with:

- `rgb565/` PNG checkpoints and matching JSON metadata;
- `usb-video/` 1920x1080 JPEG captures from the physical HDMI path.

The receipt records the downloaded release identity, installed runtime,
catalog timings, launch-return result, and file hashes. Evidence stays local;
it is not uploaded to GitHub and does not gate alpha publication.
