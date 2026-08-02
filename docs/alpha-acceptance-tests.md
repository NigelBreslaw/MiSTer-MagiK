# Local Alpha Device Tests

Physical MiSTer testing is an attended local operation. GitHub builds and
publishes the rolling `alpha` release, but it does not own or schedule access to
the developer's MiSTer.

The local journey uses the published alpha assets, the official MiSTer
Downloader, the real production UI, a real core launch and return, RGB565
checkpoints, and the fixed `USB Video` capture input.

## Run the journey

Create fresh local directories, download the current rolling alpha assets, and
run the typed workflow:

```bash
candidate_dir="$(mktemp -d)"
evidence_dir="$(mktemp -d)/alpha-evidence"
gh release download alpha \
  --repo NigelBreslaw/MiSTer-MagiK \
  --dir "$candidate_dir"
scripts/agent alpha accept \
  --candidate "$candidate_dir" \
  --output "$evidence_dir"
```

The command verifies every downloaded checksum and packaged component, installs
the rolling `alpha` through Downloader, verifies the installed runtime against
the downloaded assets, and then exercises the physical device. By default the
MiSTer remains on the tested alpha.

Use `--restore-host-mode` when the original Main selection must be restored
after the journey. Use `--reuse-installed` to repeat the journey against the
same already-installed alpha without another Downloader run or reboot; it
cannot be combined with `--restore-host-mode`.

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
