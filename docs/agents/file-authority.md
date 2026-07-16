# File authority and regeneration

Consult this table before editing or staging an unfamiliar file.

| Path pattern | Classification | Authoritative input | Regeneration command | Staging policy | Validation |
|---|---|---|---|---|---|
| `magik-gui/src/**/*.rs`, `tools/**/src/**/*.rs`, `desktop/src/**/*.rs`, `framebuffer-stream/src/**/*.rs` | Hand-edited Rust | The file itself | None | Stage intentional source edits | `scripts/validate paths PATH...` |
| `magik-gui/catalog/src/**/*.rs` | Hand-edited catalog Rust | The file itself | None | Stage intentional source edits | Catalog tests plus consumer checks selected by validation |
| `magik-gui/ui/**/*.slint`, `desktop/ui/**/*.slint` | Hand-edited Slint | The `.slint` source | Cargo/Slint build | Stage source, never generated Rust | Production UI or desktop compiled check |
| `magik-gui/ui-generated/{Cargo.toml,build.rs,src/lib.rs}` | Hand-edited generation glue | These tracked files and `magik-gui/ui/` | Cargo build | Stage only intentional glue changes | All-public format and UI check |
| Cargo `OUT_DIR/*.rs` for Slint | Build-generated | `.slint` files and `ui-generated/build.rs` | `scripts/dev-rust check-ui` | Never stage | Rebuild from authoritative inputs |
| `scripts/**/*.sh`, `scripts/**/*.py`, and extensionless executable entrypoints such as `scripts/validate`, `scripts/doctor`, `scripts/dev-rust`, and `scripts/mister` | Hand-edited tooling | The file itself | None | Stage focused script changes | `scripts/test-host-tools.sh --fast` or `--full` |
| `documentation/src/**`, `docs/**` | Hand-edited documentation | The Markdown/MDX/config source | `corepack pnpm --dir documentation run build` | Stage source; not `documentation/dist/` | Documentation build |
| `kernel/scanout-slots/**`, `fpga/**` | Hand-edited platform source | C/headers/RTL/project inputs | Approved kernel/FPGA build scripts | Never stage generated modules/RBFs outside release evidence | Contract checks plus platform qualification |
| `magik-gui/catalog/data/core_launch_manifest.json` | Checked-in generated manifest | Installed-core evidence and harvest policy | `python3 scripts/media/harvest-core-launch-manifest.py --help` | Stage generator and regenerated manifest together | Catalog tests/full host |
| `magik-gui/licenses/RUST-LIBRARIES.txt` | Checked-in generated legal inventory | Cargo locks and release features | `python3 scripts/release/packaging/generate-third-party-licenses.py` | Stage when dependency/release inventory changes | Release/host checks |
| `documentation/public/screenshots/**` | Checked-in generated documentation media | Running UI and capture scenario | `documentation/scripts/capture-guide-screenshots.sh` | Stage only intentional reviewed captures and metadata | Documentation build plus visual review |
| `**/Cargo.lock` | Checked-in dependency resolution | Corresponding `Cargo.toml` and Cargo resolver | `cargo generate-lockfile --manifest-path PATH/Cargo.toml` | Stage only with dependency/feature changes | Matching crate tests and Clippy |
| `build/`, `dist/`, `outputs/`, `**/target/`, `documentation/dist/` | Ignored generated output | Source and build commands | Re-run producing command | Never stage | Disposable |
| `history/**` | Checked-in curated evidence | Completed experiment and its provenance | Experiment-specific | Stage only deliberate dated evidence; excluded from default search | Evidence-specific checks |
| `desktop/vendor/**` | Public submodules | Upstream submodule repositories | `git submodule update --init ...` | Stage only gitlink updates | Desktop checks |
| `private/magik-cloud/**` | Private submodule | Private repository | `scripts/magik-cloud run -- ...` | Commit/push private repo first; parent stages only gitlink | Private submodule checks |
| `private/test-fixtures/**` | Ignored local fixtures | Local device/library data | Manual/local | Never stage | Optional local validation only |
| `.env*`, `.wrangler/**`, credentials, tokens | Ignored secrets | Local secret manager/environment | Never regenerate into repo | Never stage or print | `git check-ignore` |
| Device paths under `/media/fat` and `/tmp/mister-magik` | Device-owned runtime state | Installed bundle/runtime | Approved `scripts/mister` or deploy command | Never copy into Git as source | Attended device checks only |

When a generated file is not listed, find its producer before editing. If no
producer can be found, stop and update this guide as part of the change rather
than guessing.
