# Private Quartus GHCR Image

This repo builds the experimental FPGA vblank-latch core with Quartus Prime
Lite 17.0 and Cyclone V support. The private GHCR image keeps that large,
licensed toolchain out of normal workflow setup and lets CI pull one pinned
`linux/amd64` image on GitHub-hosted x64 Linux runners.

Image package:

```text
ghcr.io/nigelbreslaw/mister-magik-quartus-lite
```

Tags:

```text
17.0.0.595-cyclonev-ubuntu18
17.0-cyclonev-ubuntu18
sha-<commit>
```

Use digest-pinned pulls for real builds. Tags are aliases for humans and for
finding the latest published image, not reproducibility anchors.

## Publishing

Run the `Quartus GHCR Image` workflow manually. It needs these repository
secrets:

```text
QUARTUS_17_0_RUN_URL
QUARTUS_17_0_CYCLONEV_QDZ_URL
```

The workflow downloads the official installer payloads with BuildKit secrets,
verifies the known SHA1s, installs Quartus into `/opt/intelFPGA_lite/17.0`,
pushes the image, pulls the pushed digest, and smoke-tests:

```text
quartus_sh --version
quartus_map --version
test -x /opt/intelFPGA_lite/17.0/quartus/bin/quartus_sh
```

After the first publish, confirm the GHCR package visibility is `Private` and
that `NigelBreslaw/MiSTer-MagiK` has Actions access to the package. GitHub's
relevant docs are:

- [Working with the Container registry](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry)
- [Configuring a package's access control and visibility](https://docs.github.com/en/packages/learn-github-packages/configuring-a-packages-access-control-and-visibility)

## Consuming From Another Workflow

Grant package read access and log in to GHCR:

```yaml
permissions:
  contents: read
  packages: read

steps:
  - uses: docker/login-action@v3
    with:
      registry: ghcr.io
      username: ${{ github.actor }}
      password: ${{ secrets.GITHUB_TOKEN }}

  - run: docker pull ghcr.io/nigelbreslaw/mister-magik-quartus-lite@sha256:<digest>
```

Then use the baked install mode in the FPGA build script:

```bash
QUARTUS_DOCKER_BAKED_INSTALL=1 \
QUARTUS_DOCKER_IMAGE=ghcr.io/nigelbreslaw/mister-magik-quartus-lite@sha256:<digest> \
scripts/build-fpga-vblank-latch-core.sh
```

The existing `.github/workflows/fpga-vblank-latch.yml` still uses the
cache-mounted installer path. A later consumer PR should replace its
`prepare-quartus` cache/install job with GHCR login plus digest pull, then run
the existing RBF build with `QUARTUS_DOCKER_BAKED_INSTALL=1`.
