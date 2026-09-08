# Apple Container build storage

MagiK retains at most **four idle build containers across checkouts** for
**two hours after their last build use**. It checks retention at build start
and finish, including cached-artifact builds. Active builds are exempt. There
is no background timer: the last warm containers can remain past expiry until
the next build or explicit cleanup.

```sh
scripts/magik storage report
scripts/magik storage report --json
scripts/magik storage clean
scripts/magik storage clean --apply
scripts/magik storage clean --all-idle
scripts/magik storage clean --all-idle --apply
```

`report` and `clean` without `--apply` only inspect resources and establish
coordination locks; they do not delete anything. `--all-idle` ignores the age
and count limits for managed containers. Images still require two hours of
non-use and no container or active-build reference. All commands are host-only:
no `MISTER_IP`, device connection, bootstrap, or deployment is required. Apple
Container must be installed and running. Agent callers require the usual
first-attempt sandbox escalation for Apple Container operations.

Reports show ownership, workspace existence, activity, last use, and deletion
eligibility. `logical_bytes` is virtual disk capacity; `allocated_bytes` counts
allocated file blocks, which can be shared by APFS clones. Neither is a promise
of recoverable space. Applied cleanup reports the measured filesystem free-space
change, which can also include concurrent disk activity. Apple-wide image,
container, and volume totals are included in JSON disk accounting.

## Coordination and preservation

Containers carry versioned ownership labels, canonical checkout identity,
recipe identity, and the shared state directory. Metadata and OS locks live
under `build-storage` within the existing MagiK state root (normally
`~/.local/state/mister-magik`). All worktrees should use the same
`MISTER_MAGIK2_STATE` override if one is configured. Containers from another
state root or management version are protected, and cannot be silently adopted.

A checkout lease spans input/cache validation, FFmpeg preparation, compilation,
and artifact publication. Same-checkout builds serialize; other checkouts may
compile concurrently. Lifecycle operations coordinate image preparation and
container creation/deletion. Cleanup never waits for an active checkout lease.
It also checks guest processes immediately before deleting a running container,
because guest compilation can survive a disconnected host command. New
containers use Apple's init process to reap children. Unknown state, mismatched
identity, unreadable metadata, or failed process inspection prevents deletion.

Source files, package `target` directories (including FFmpeg), and shared host
Cargo registry/git caches remain mounted from the host. Cleanup never removes
these directories, shared BuildKit, legacy images, or named volumes. Failed
build containers get the same retention period as successful builds. Automatic
cleanup failures are emitted as storage warnings without replacing the build
result. Explicit storage commands exit nonzero on incomplete inspection or
failed deletion, and report completed actions alongside failures. Ambiguous
mutations are reconciled against inventory, never blindly replayed.

Unlabeled old `magik-*` containers appear as migration candidates. They are
excluded from automatic cleanup; updating the tooling creates a separately
named managed container instead of taking over an old one.

Recognized `magik2-v1-*` containers and valid `magik2-build:*` image records are
shown as `legacy-retained`, not ownership errors. They are never adopted, stopped,
or deleted by this manager, including `clean --all-idle --apply`. Inspect them and
arrange explicit cleanup separately. Malformed or changed image metadata still
reports an error and remains protected.

## Initial and occasional maintenance

Use a quiet build window for legacy resources and the shared image builder.
These resources do not participate in the new build-lease protocol.

1. Save `container system df --format json`, `container list --all --format json`,
   `container image list`, `container volume list`, `df -k`, and allocated sizes
   under `~/Library/Application Support/com.apple.container`.
2. Match legacy containers to their exact workspace mounts and toolchain image.
   Inspect host build clients and guest processes. Recheck immediately before
   `container stop NAME` and `container delete NAME`; skip active or uncertain
   resources. Do not infer idleness just from a missing checkout directory.
3. When no image build or builder worker is active, use `container builder stop`
   followed by `container builder delete` to discard BuildKit cache. Future
   image builds recreate it. Never reset the shared builder automatically.
4. Delete obsolete images individually with `container image delete REFERENCE`,
   checking all container references/digests first. Keep the current checkout's
   `magik-build` recipe image and required Apple runtime images. Avoid a global
   image prune that also removes unrelated users' resources.
5. Before `container volume delete NAME`, verify the volume is unreferenced and
   contains only regenerable scratch state. For Quartus, inspect it read-only
   using a disposable container. Preserve the host-installed toolchain and
   downloaded installers. Do not remove a volume whose contents are uncertain.
6. Record the final inventory and actual free-space change. Never delete
   Apple's internal snapshots, content blobs, or sparse disk files directly.

Removed legacy images require downloads/rebuilds when those workflows are next
used. Removed Quartus scratch roots require chroot regeneration; this does not
remove the separately mounted host installation.
