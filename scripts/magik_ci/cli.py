# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any, cast

from . import architecture, build, bundle, databases, host, metadata, quality
from .common import github_output, repository_root


def parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="magik-ci")
    sub = parser.add_subparsers(dest="group", required=True)
    architecture_parser = sub.add_parser("architecture")
    architecture_sub = architecture_parser.add_subparsers(dest="command", required=True)
    report = architecture_sub.add_parser("report")
    report.add_argument("--base", required=True)
    report.add_argument("--head", required=True)
    report.add_argument("--format", choices=("json", "markdown"), default="json")
    report.add_argument("--output", type=Path)
    build_parser = sub.add_parser("build")
    build_parser.add_argument(
        "intent", choices=(*build.COMMANDS, *build.CHECKS, "release-binaries")
    )
    quality_parser = sub.add_parser("quality")
    quality_parser.add_argument(
        "checks", nargs="+", choices=("format", "lint", "typecheck", "all")
    )
    ci = sub.add_parser("ci")
    ci_sub = ci.add_subparsers(dest="command", required=True)
    distribution = ci_sub.add_parser("distribution")
    distribution_sub = distribution.add_subparsers(dest="action", required=True)
    distribution_verify = distribution_sub.add_parser("verify")
    distribution_verify.add_argument("candidate", type=Path)
    distribution_verify.add_argument(
        "--channel", required=True, choices=("alpha", "beta", "release")
    )
    distribution_verify.add_argument("--write-receipt", action="store_true")
    delivery_test = distribution_sub.add_parser("test-delivery")
    delivery_test.add_argument("candidate", type=Path)
    delivery_test.add_argument(
        "--channel", required=True, choices=("alpha", "beta", "release")
    )
    delivery_test.add_argument("--downloader-source", type=Path, required=True)
    delivery_test.add_argument(
        "--device-downloader-source",
        type=Path,
        required=True,
        help="device-compatible pinned Downloader source used by cached fallback tests",
    )
    delivery_test.add_argument(
        "--native-downloader",
        type=Path,
        required=True,
        help="hash-checked ARMv7 native Downloader runtime",
    )
    delivery_test.add_argument(
        "--update-all-source",
        type=Path,
        required=True,
        help="real pinned Update All pyz used for the second delivery entrypoint",
    )
    for action in ("prepare-promotion", "publish"):
        command = distribution_sub.add_parser(action)
        command.add_argument("candidate", type=Path)
        command.add_argument(
            "--channel", required=True, choices=("alpha", "beta", "release")
        )
        command.add_argument("--repository", required=True)
        command.add_argument("--source-revision", required=True)
        if action == "prepare-promotion":
            command.add_argument("--timestamp", required=True, type=int)
    assurance = ci_sub.add_parser("host-assurance")
    assurance_scope = assurance.add_mutually_exclusive_group(required=True)
    assurance_scope.add_argument("--paths", nargs="+")
    assurance_scope.add_argument("--group", dest="host_group", choices=host.HOST_GROUPS)
    candidates = ci_sub.add_parser("platform-candidates")
    candidates.add_argument("artifacts", type=Path)
    candidates.add_argument("name")
    eligible = ci_sub.add_parser("platform-eligible-run")
    eligible.add_argument("run", type=Path)
    eligible.add_argument("head_sha")
    eligible.add_argument("--allow-failed", action="store_true")
    pm = ci_sub.add_parser("platform-manifest")
    pm_sub = pm.add_subparsers(dest="action", required=True)
    pm_gen = pm_sub.add_parser("generate")
    pm_gen.add_argument("--output", type=Path, required=True)
    pm_gen.add_argument("--main", type=Path, required=True)
    pm_gen.add_argument("--gui", type=Path, required=True)
    pm_gen.add_argument("--manager", type=Path, required=True)
    pm_gen.add_argument("--scanout-module", type=Path, required=True)
    pm_gen.add_argument("--scanout-metadata", type=Path, required=True)
    pm_gen.add_argument("--latch-rbf", type=Path, required=True)
    pm_gen.add_argument("--latch-metadata", type=Path, required=True)
    pm_gen.add_argument("--release-version", type=int)
    pm_gen.add_argument("--bundle-id")
    pm_gen.add_argument("--platform-bundle-manifest", type=Path)
    pm_gen.add_argument("--main-revision", required=True)
    pm_gen.add_argument("--magik-revision", required=True)
    pm_gen.add_argument("--layout", required=True, choices=("public", "dev"))
    pm_verify = pm_sub.add_parser("verify")
    pm_verify.add_argument("manifest", type=Path)
    pm_verify.add_argument("--root", type=Path)
    pm_verify.add_argument("--layout", required=True, choices=("public", "dev"))
    db = ci_sub.add_parser("game-databases")
    db_sub = db.add_subparsers(dest="action", required=True)
    db_verify = db_sub.add_parser("verify")
    db_verify.add_argument("archive", type=Path)
    db_verify.add_argument("--manifest", type=Path)
    db_verify.add_argument("--checksums", type=Path)
    db_verify.add_argument("--release-version", type=int)
    extract = db_sub.add_parser("extract-release")
    extract.add_argument("release", type=Path)
    extract.add_argument("--output", type=Path, required=True)
    db_plan = db_sub.add_parser("plan-update")
    db_plan.add_argument("--manifest", type=Path)
    db_plan.add_argument("--mame-tag", required=True)
    db_plan.add_argument("--mame-sha", required=True)
    db_plan.add_argument("--hbmame-tag", required=True)
    db_plan.add_argument("--hbmame-sha", required=True)
    db_plan.add_argument("--arcade-database-sha", required=True)
    db_plan.add_argument("--arcade-updater-builder-sha", required=True)
    db_plan.add_argument("--arcade-updater-revision", action="append", default=[])
    db_plan.add_argument("--github-output", type=Path)
    mame = db_sub.add_parser("build-mame")
    mame.add_argument("--listxml", type=Path, required=True)
    mame.add_argument("--out", type=Path, required=True)
    mame.add_argument("--software-dir", type=Path)
    mame.add_argument("--mame", type=Path)
    mame.add_argument("--machine-sqlite", type=Path)
    mame.add_argument("--runtime-coverage-output", type=Path)
    import_arcade = db_sub.add_parser("import-arcade")
    import_arcade.add_argument("--sqlite", type=Path, required=True)
    import_arcade.add_argument("--csv", type=Path, required=True)
    import_arcade.add_argument("--source-sha", required=True)
    create_db = db_sub.add_parser("create")
    aliases = {
        "mame-sqlite": "mame",
        "hbmame-sqlite": "hbmame",
        "mame-listxml-asset": "listxml_asset",
        "mame-listxml-sha256": "listxml_sha256",
        "arcade-database-csv": "arcade_database_csv",
        "arcade-database-license": "arcade_database_license",
        "arcade-updater-index": "arcade_updater_index",
    }
    for option, destination in aliases.items():
        create_db.add_argument(
            f"--{option}",
            dest=destination,
            type=Path
            if destination
            in {
                "mame",
                "hbmame",
                "arcade_database_csv",
                "arcade_database_license",
                "arcade_updater_index",
            }
            else str,
            required=True,
        )
    create_db.add_argument("--runtime-metadata", type=Path)
    create_db.add_argument("--source-output", type=Path)
    for option in (
        "release-version",
        "mame-tag",
        "mame-sha",
        "hbmame-tag",
        "hbmame-sha",
        "arcade-database-sha",
        "arcade-database-builder-sha",
        "arcade-updater-builder-sha",
        "mame-builder-sha",
        "hbmame-builder-sha",
    ):
        create_db.add_argument(
            f"--{option}",
            required=True,
            type=int if option == "release-version" else str,
        )
    create_db.add_argument("--output", type=Path, required=True)
    updater = db_sub.add_parser("build-updater-arcade")
    updater.add_argument("--input-manifest", type=Path, required=True)
    updater.add_argument("--out", type=Path, required=True)
    pb = ci_sub.add_parser("platform-bundle")
    pb_sub = pb.add_subparsers(dest="action", required=True)
    pb_plan = pb_sub.add_parser("plan-update")
    pb_plan.add_argument("--manifest", type=Path)
    pb_plan.add_argument("--current-version", type=int, required=True)
    pb_plan.add_argument("--main-id", required=True)
    pb_plan.add_argument("--fpga-id", required=True)
    pb_plan.add_argument("--kernel-id", required=True)
    pb_plan.add_argument("--github-output", type=Path)
    pb_verify = pb_sub.add_parser("verify")
    pb_verify.add_argument("archive", type=Path)
    pb_verify.add_argument("--manifest", type=Path)
    pb_verify.add_argument("--release-version", type=int)
    pb_verify.add_argument("--historical-baseline", action="store_true")
    pb_extract = pb_sub.add_parser("extract-component")
    pb_extract.add_argument("archive", type=Path)
    pb_extract.add_argument("--manifest", type=Path, required=True)
    pb_extract.add_argument("--component", required=True)
    pb_extract.add_argument("--component-id", required=True)
    pb_extract.add_argument("--output", type=Path, required=True)
    pb_extract.add_argument("--historical-baseline", action="store_true")
    pb_cache = pb_sub.add_parser("write-component-cache")
    pb_cache.add_argument("--component", required=True)
    pb_cache.add_argument("--artifact", type=Path, required=True)
    pb_cache.add_argument("--component-id", required=True)
    pb_cache.add_argument("--run-id", required=True)
    pb_cache.add_argument("--head-sha", required=True)
    pb_vc = pb_sub.add_parser("verify-component")
    pb_vc.add_argument("--component", required=True)
    pb_vc.add_argument("--artifact", type=Path, required=True)
    pb_vc.add_argument("--component-id", required=True)
    pb_vc.add_argument("--revision")
    pb_compact = pb_sub.add_parser("compact-component")
    pb_compact.add_argument("--component", required=True)
    pb_compact.add_argument("--artifact", type=Path, required=True)
    pb_compact.add_argument("--output", type=Path, required=True)
    pb_compact.add_argument("--component-id", required=True)
    pb_create = pb_sub.add_parser("create")
    for name in ("main-dir", "fpga-dir", "scanout-dir", "output"):
        pb_create.add_argument(f"--{name}", type=Path, required=True)
    for name in (
        "main-id",
        "fpga-id",
        "kernel-id",
        "main-run-id",
        "fpga-run-id",
        "kernel-run-id",
        "main-head-sha",
        "fpga-head-sha",
        "kernel-head-sha",
        "main-source",
        "fpga-source",
        "kernel-source",
    ):
        pb_create.add_argument(f"--{name}", required=True)
    pb_create.add_argument("--release-version", type=int, required=True)
    return parser


def main() -> int:
    args = parser().parse_args()
    root = repository_root()
    if args.group == "architecture":
        architecture.execute(root, args)
    elif args.group == "build":
        build.execute(root, args.intent)
    elif args.group == "quality":
        quality.execute(root, args.checks)
    elif args.group == "ci":
        if args.command == "host-assurance":
            if args.host_group:
                host.execute(root, args.host_group)
            else:
                metadata.host_assurance(args.paths)
        elif args.command == "platform-candidates":
            for item in metadata.platform_candidates(args.artifacts, args.name):
                item_data = cast(dict[str, Any], item)
                origin = cast(dict[str, Any], item_data.get("workflow_run", {}))
                print(
                    f"{item_data.get('id', '')}\t{origin.get('id', '')}\t{origin.get('head_sha', '')}"
                )
        elif args.command == "platform-eligible-run":
            raise SystemExit(
                0
                if metadata.platform_eligible_run(
                    args.run, args.head_sha, allow_failed=args.allow_failed
                )
                else 1
            )
        elif args.command == "distribution":
            from . import delivery_tests, distribution, publication

            if args.action == "test-delivery":
                result = delivery_tests.run(
                    args.candidate,
                    channel=args.channel,
                    source=args.downloader_source,
                    device_source=args.device_downloader_source,
                    native_downloader=args.native_downloader,
                    update_all_source=args.update_all_source,
                )
            elif args.action == "publish":
                result = publication.publish(
                    args.candidate,
                    channel=args.channel,
                    github=publication.GitHub(args.repository),
                    source_revision=args.source_revision,
                )
            elif args.action == "prepare-promotion":
                result = publication.prepare_promotion(
                    args.candidate,
                    channel=args.channel,
                    repository=args.repository,
                    source_revision=args.source_revision,
                    timestamp=args.timestamp,
                )
            else:
                result = distribution.verify(
                    args.candidate,
                    channel=args.channel,
                    write_receipt=args.write_receipt,
                )
            print(json.dumps(result, sort_keys=True))
        elif args.command == "platform-manifest":
            from . import manifest

            if args.action == "generate":
                bundle_payload = (
                    cast(
                        dict[str, Any],
                        json.loads(args.platform_bundle_manifest.read_text()),
                    )
                    if args.platform_bundle_manifest
                    else {}
                )
                manifest.generate(
                    args.output,
                    {
                        "main": args.main,
                        "gui": args.gui,
                        "manager": args.manager,
                        "scanout_module": args.scanout_module,
                        "scanout_metadata": args.scanout_metadata,
                        "latch_rbf": args.latch_rbf,
                        "latch_metadata": args.latch_metadata,
                    },
                    release_number=args.release_version
                    or int(bundle_payload["release_version"]),
                    bundle_id=args.bundle_id or str(bundle_payload["bundle_id"]),
                    main_revision=args.main_revision,
                    magik_revision=args.magik_revision,
                    layout=args.layout,
                )
            else:
                manifest.verify(args.manifest, args.root, layout=args.layout)
        elif args.command == "game-databases":
            if args.action == "verify":
                databases.verify(
                    args.archive, args.manifest, release_version=args.release_version
                )
            elif args.action == "extract-release":
                databases.extract_release(args.release, args.output)
            elif args.action == "plan-update":
                current = (
                    json.loads(args.manifest.read_text()) if args.manifest else None
                )
                value = databases.update_plan(
                    current,
                    mame_tag=args.mame_tag,
                    mame_sha=args.mame_sha,
                    hbmame_tag=args.hbmame_tag,
                    hbmame_sha=args.hbmame_sha,
                    arcade_database_sha=args.arcade_database_sha,
                    arcade_updater_builder_sha=args.arcade_updater_builder_sha,
                    revisions=args.arcade_updater_revision,
                )
                github_output(args.github_output, value)
                print(json.dumps(value, sort_keys=True))
            elif args.action == "build-mame":
                databases.build_mame(
                    listxml=args.listxml,
                    out=args.out,
                    software_dir=args.software_dir,
                    mame=args.mame,
                    machine_sqlite=args.machine_sqlite,
                )
                if args.runtime_coverage_output:
                    coverage = databases.mame_runtime_coverage(args.out)
                    args.runtime_coverage_output.parent.mkdir(
                        parents=True, exist_ok=True
                    )
                    args.runtime_coverage_output.write_text(
                        json.dumps(coverage, indent=2, sort_keys=True) + "\n",
                        encoding="utf-8",
                    )
                    print(json.dumps(coverage, sort_keys=True))
            elif args.action == "import-arcade":
                summary = databases.import_arcade_database(
                    sqlite=args.sqlite,
                    csv_path=args.csv,
                    source_sha=args.source_sha,
                )
                print(json.dumps(summary, sort_keys=True))
            elif args.action == "create":
                databases.create(
                    mame=args.mame,
                    hbmame=args.hbmame,
                    release_version=args.release_version,
                    mame_tag=args.mame_tag,
                    mame_sha=args.mame_sha,
                    listxml_asset=args.listxml_asset,
                    listxml_sha256=args.listxml_sha256,
                    hbmame_tag=args.hbmame_tag,
                    hbmame_sha=args.hbmame_sha,
                    mame_builder_sha=args.mame_builder_sha,
                    hbmame_builder_sha=args.hbmame_builder_sha,
                    arcade_database_csv=args.arcade_database_csv,
                    arcade_database_license=args.arcade_database_license,
                    arcade_database_sha=args.arcade_database_sha,
                    arcade_database_builder_sha=args.arcade_database_builder_sha,
                    arcade_updater_builder_sha=args.arcade_updater_builder_sha,
                    arcade_updater_index=args.arcade_updater_index,
                    output=args.output,
                    runtime_metadata=args.runtime_metadata,
                    source_output=args.source_output,
                )
            else:
                value = databases.build_updater(args.input_manifest, args.out)
                print(json.dumps(value, sort_keys=True))
        elif args.command == "platform-bundle":
            if args.action == "plan-update":
                current = (
                    json.loads(args.manifest.read_text()) if args.manifest else None
                )
                value = bundle.update_plan(
                    current,
                    args.current_version,
                    args.main_id,
                    args.fpga_id,
                    args.kernel_id,
                )
                github_output(args.github_output, value)
                print(json.dumps(value, sort_keys=True))
            elif args.action == "verify":
                bundle.verify(
                    args.archive,
                    args.manifest,
                    args.release_version,
                    historical_baseline=args.historical_baseline,
                )
            elif args.action == "extract-component":
                print(
                    json.dumps(
                        bundle.extract_component(
                            args.archive,
                            args.manifest,
                            args.component,
                            args.component_id,
                            args.output,
                            historical_baseline=args.historical_baseline,
                        ),
                        sort_keys=True,
                    )
                )
            elif args.action == "write-component-cache":
                bundle.write_component_cache(
                    args.component,
                    args.artifact,
                    args.component_id,
                    args.run_id,
                    args.head_sha,
                )
            elif args.action == "verify-component":
                print(
                    json.dumps(
                        bundle.verify_component(
                            args.component,
                            args.artifact,
                            args.component_id,
                            args.revision,
                        ),
                        sort_keys=True,
                    )
                )
            elif args.action == "compact-component":
                bundle.compact_component(
                    args.component, args.artifact, args.output, args.component_id
                )
            elif args.action == "create":
                print(
                    bundle.create(
                        main=args.main_dir,
                        fpga=args.fpga_dir,
                        scanout=args.scanout_dir,
                        main_id=args.main_id,
                        fpga_id=args.fpga_id,
                        kernel_id=args.kernel_id,
                        main_run_id=args.main_run_id,
                        fpga_run_id=args.fpga_run_id,
                        kernel_run_id=args.kernel_run_id,
                        main_head_sha=args.main_head_sha,
                        fpga_head_sha=args.fpga_head_sha,
                        kernel_head_sha=args.kernel_head_sha,
                        main_source=args.main_source,
                        fpga_source=args.fpga_source,
                        kernel_source=args.kernel_source,
                        release_version=args.release_version,
                        output=args.output,
                    )
                )
    return 0
