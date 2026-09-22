# Historical evidence

Historical incident records, qualification summaries, compact catalog measurement
tables, and the source-hashed compile reports referenced by current documentation
remain here. They are dated evidence, not current operating instructions.

Retired June/July benchmark streams, two full-motion profiling bundles, obsolete
Slint fixtures, and the first-working screenshot were removed from the checkout.
No Git history was rewritten. Do not use their historical frame or CPU readings
as evidence for the current launcher.

## Recovery inventory

Original source commit: `3b8d2c3f2d38278b9319fadcddaa40ccb477901d`. Paths below are relative to `history/`.
Recover any artifact without checking out the old tree, for example:

```sh
git show 3b8d2c3f2d38278b9319fadcddaa40ccb477901d:history/toolchain-bench/results.tsv
```

Each original was checked byte-for-byte against this commit before deletion.
The inventory records SHA-256 and byte count for verification after recovery.

| Removed artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `2026-5-2/first-working-image.png` | 32860 | `9b7cf0a918db8e94f786b350257d62b3aa8ff49aad7e691cedf87b3ad8c5bfd3` |
| `bench-scenes/2026-06-retired-slint-scenes/demo.slint` | 2036 | `31c1e97181a1d1a053cd14324581a01f23a2957242e34e6baa30605a27b819c5` |
| `bench-scenes/2026-06-retired-slint-scenes/full_motion.slint` | 1829 | `d7e574deccde1fb83e41113cc0b3ee2b6240d2852703b122880610f3e7bdc93c` |
| `bench-scenes/2026-06-retired-slint-scenes/local_motion.slint` | 1020 | `706e96fd78b78c46ac98e91c8d6eea682290d9b8d69711086d0a28efcbc5d991` |
| `bench-scenes/2026-06-retired-slint-scenes/static_ui.slint` | 1245 | `d0b54dd0aa031fef1490f91fbc6a1117a9edc84971b46a789d618d5699f97f30` |
| `toolchain-bench/profile-full_motion-20260604T191046Z/device.log` | 4165 | `463db05823a6499192dc21ff62e42e36a0c800ffd9bfd7bfabe914d335004891` |
| `toolchain-bench/profile-full_motion-20260604T191046Z/mister-frame-full_motion-20260604T191046Z.tsv` | 22853 | `2dfc53b1dcd0e99c4822a2d57e70ad7a8a7d39ee58cd33ba302ad6ce6ef36564` |
| `toolchain-bench/profile-full_motion-20260604T191046Z/run.meta` | 99 | `eee8f218ffa464349e4074ee0d3fba08b3133f89e607f271cbb5573e52c14176` |
| `toolchain-bench/profile-full_motion-20260604T191046Z/ui.log` | 4125 | `0f2380dffcad09bf29dccff97d3ded5bf45bc9ba6475dcbf11340733e9f65758` |
| `toolchain-bench/profile-full_motion-20260604T191823Z/analysis.txt` | 752 | `61357bb4e7460700f3ab99f1efe4c29524148b40975424074ad216966c2082ae` |
| `toolchain-bench/profile-full_motion-20260604T191823Z/device.log` | 3456 | `5716d42a856b868e1ad9ca25445a3f25561267f3ae5d4d35b7e212a2b1c4fd4f` |
| `toolchain-bench/profile-full_motion-20260604T191823Z/mister-frame-full_motion-20260604T191823Z.tsv` | 12836 | `1e6322eec3294be5557e8017664756bd5995ea1532bbd796a71b49ccc6f4bc17` |
| `toolchain-bench/profile-full_motion-20260604T191823Z/run.meta` | 99 | `a78b21e71fef9ec7d6e6bd98b6a7699c3ef1b0dc7e43e43c0e9dbc759a0b0e2a` |
| `toolchain-bench/profile-full_motion-20260604T191823Z/ui.log` | 3416 | `8d889864b6c0119eb93db3d730ed0d26d7aa80f72a6f6ebe5d57bdc80c744f6a` |
| `toolchain-bench/results-agent.tsv` | 44579 | `ad3cabe356114c66aed70b183fe60c0d748612e402276c6914cda8fc974ce17f` |
| `toolchain-bench/results-boot-net.tsv` | 8619 | `541cbfa17b5c7035d85cff668fd0701f7b7e09abce03e49ce8a2538cb85662ed` |
| `toolchain-bench/results-boot-tcp.tsv` | 2175 | `a36b2e28c0486c4c76ab0c872b32b17c4b137fee399b3f31017eba022277af42` |
| `toolchain-bench/results-camera-effects.tsv` | 28487 | `5eec461beca80dac3e3a3da445346f35587ee26be23422859044fc51223d71e7` |
| `toolchain-bench/results-catalog-destruction.tsv` | 3276 | `f6fbe89b3e56086de13d2907af4e78cca70a0f0a90b320c38dcc5d0c431ec0f0` |
| `toolchain-bench/results-catalog-drift-acceptance.tsv` | 5228 | `73947dd4ca23dc6202598e989bb0ee891f681ca4a5260e700f2f9f2a2cd5463f` |
| `toolchain-bench/results-catalog.tsv` | 594 | `a47b82b438d9bac42523ae75582a83df4ef36888918d7d59822ba835b1f0bc5f` |
| `toolchain-bench/results-connection-profile.tsv` | 726 | `71f60c0d0652c749c5544aab256936a2d93a05f4977da60314cb79ad16664864` |
| `toolchain-bench/results-effects.tsv` | 7895 | `e6d3221bb28d567e9e2d930e9e9cb2a1faebd5f13967b3df51ba63a4068e0c3f` |
| `toolchain-bench/results-first-scan.tsv` | 4639940 | `8acefeb5a30d49a04bc3129ce6ada207ee3baa3ec9e9db7c9b2a386e113f2cf9` |
| `toolchain-bench/results-fs-fault-reset.tsv` | 11782 | `dd266fe308c8355f0f56beb0ab01d54f23ce0ce2e6c3c26903752505b081525b` |
| `toolchain-bench/results-launch-handoff.tsv` | 7195 | `2c69798081ff07cbfb1166a5652c407235eb3dadeb94689e36008f3bd5e09592` |
| `toolchain-bench/results-launch-prep.tsv` | 946616 | `bb04d0db0293cb99151767c786719c07071cc1323956e17b140b1367a848ba4d` |
| `toolchain-bench/results-library-change-flow.tsv` | 929 | `615f8542cebf4c2d80cbedd3196e01e6838d308d4b02dfcb5667d0791d58414b` |
| `toolchain-bench/results-library-db-query.tsv` | 10749 | `9daaa6aa7348e528a4e3eda191761b74a2dd32cf2d5072e8442a88cde6ea339b` |
| `toolchain-bench/results-library-io.tsv` | 602290 | `f271a88cf7d85559672f84c9eebe522410dd5ff7d3d9657acf0b3db613a89fdf` |
| `toolchain-bench/results-library-save.tsv` | 5072 | `37c9bf477491e381bd153792db87f81591a556e4492c67bb1cd3997dbfbfbb4f` |
| `toolchain-bench/results-library.tsv` | 195403 | `bef497ae18140f443f01e87d412b99047c631bde0de5a05a0c2b4842b5e63228` |
| `toolchain-bench/results-media-cold-boot.tsv` | 1034992 | `bc8bdf7db88b8ee2787de349f7f12106cb34cad97e97ba2ccec0bef134ec563f` |
| `toolchain-bench/results-raster-effects.tsv` | 26703 | `ac841b540d2df78f0ccac0d5e35d07f32241674ffe8a76cc049b41030dc685dd` |
| `toolchain-bench/results-screenshot-download.tsv` | 275495 | `1677bc6a3ccbec2e7d7ff877354784aa6e8b6d9191d5ec0995415ad8b0ba4ac3` |
| `toolchain-bench/results-screenshot-save.tsv` | 17278 | `8b3c80319058f190eab154d66bbfef0ba6a074ab6f2b6a599568586a6329aec0` |
| `toolchain-bench/results-sprite-effects.tsv` | 19462 | `0056f9471cfab41d6d865d4571f4977e4709076e6140f0bfab1a0a9112bf705d` |
| `toolchain-bench/results-startup-reveal.tsv` | 1328 | `bac38c821b31c6efcdf0b7da57419531f2cbbf41f43c394d3ed955be545dfa52` |
| `toolchain-bench/results-text-effects.tsv` | 84437 | `6fa01760178b8b6a18e07a0e072951faefb58b1d14d50d5495ecd162c424bf2c` |
| `toolchain-bench/results-transition-effects.tsv` | 30275 | `d420daf909e672f9b4d0cf86bb036ea566c1040b401e512c65bc94ba13672879` |
| `toolchain-bench/results-warm-catalog.tsv` | 29785 | `6f496aad3e2e86dc345498f605bd672269848fc82505811427d6f242281fcedb` |
| `toolchain-bench/results.tsv` | 179081 | `117c877806a3abcf4272d01d61d0e18ba7016570fd25972d34697bc6d27ee183` |
