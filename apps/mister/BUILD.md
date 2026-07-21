# MiSTer frontend build policy

Agents build and validate through `scripts/agent`; they do not invoke Cargo,
Apple container, cross, or deployment implementation commands directly.

The typed build state machine is:

```text
Infer → Preflight → PrepareContainer → Compile → Verify → Receipt → Complete
```

`BuildSpec` infers profile, UI scope, production features, toolchain, artifact,
and cache identity from changed components and delivery intent. Apple Silicon
uses Apple `container`; Linux CI may use the explicitly typed cross adapter.
There is no automatic backend fallback.

Every artifact receipt records the exact clean source SHA, lock/toolchain/cache
identity, target, profile, features, and output digest. Runtime and platform
delivery reject mismatched or dirty receipts.

Use:

```text
scripts/agent check
scripts/agent verify
scripts/agent commit -m "Describe the change"
scripts/agent deliver
```

`deliver` owns build scope, artifact qualification, transport, activation,
rollback, and smoke verification. Human-only fixed scene operation is available
through `mister scene`; it is separate from building and deployment.
