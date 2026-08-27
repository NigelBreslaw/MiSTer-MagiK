# Schema-11 scaler-fetch diagnostic deployment

## Attended transaction result

On 2026-08-27 the signed schema-11
`scaler-fetch-ordered-signature-v1` diagnostic was installed in the Dev layout
through the rollback-capable attended FPGA transaction. No raw device copy or
stock `menu.rbf` activation route was used.

The first activation attempt failed closed because the installed device agent
did not yet decode schema 11. The FPGA transaction reported
`rollback=complete` and restored the preceding schema-10 experimental RBF,
whose Dev-manifest SHA-256 identity was
`6c9324ffe52cd6ae10ecac31ef8a37a281dd2882156ecdc4571e5ab5d35a7691`.

The current ARM device agent was then built canonically and installed through
the separate attended experimental-agent transaction, bound to that exact
schema-10 RBF identity. Its SHA-256 is
`c2f62a3ebe91e8d7a7d0bfc155e9971454a043c87043241f268509dee3d6a421`.
The transaction used one bounded reboot, verified compatibility with the
installed schema-10 diagnostic, and committed successfully. The schema-11 FPGA
transaction was then retried and committed successfully with these exact
signed inputs:

- RBF: `f0c80706681cdc3126a0bd26dc089b0f4576a0ffba42f3a5088420feb1dce60e`;
- metadata: `c915b0566443f165554e47f9f79b8fe1c6bad0de34b26ec2866c6da6207d4d6d`;
- delta signoff report:
  `a5c9cf0c47b4ae4527995c45eb0e78e620eac02f96e7a3ba092e328d961a8c51`.

## Independent diagnostic smoke

After the transaction committed, typed device status reported
`MiSTer_MagiKDev`, `LauncherActive`, launcher `ready`, attempt 1, and no last
failure. A separate typed diagnostic collection returned:

- architecture `scaler-fetch-ordered-signature-v1` and capability
  `scaler_fetch_ordered_signature=true`;
- `available=true`, `coherent=true`, and `three_samples_valid=true`;
- capture sequences `1455`, `1456`, and `1458`;
- valid flags `1,1,1` and fault flags `0,0,0`;
- ordered signatures `add7,add7,add7`;
- classification `scaler_fetch_ordered_stable`.

The ignored bundle is under
`build/agent-diagnostics/2026-08-27-schema11-postinstall-smoke/`. Its
`bundle.json` SHA-256 is
`062abc7df87f5890e5f0833937ee39413795fd017f0d5c42215ea5cdf1e89f0a`.

## Physical-capture limitation

The accompanying 15.015-second, 375-frame, 1920x1080 USB movie returned the
fixed eight-bar unavailable-input pattern. The operator then confirmed that
USB video was not connected, matching the repository's prior capture-chain
control. The movie SHA-256 is
`297f33087c078feaba552a42ae9e4185962ed4063f2386ead238099f1f3c16a4`.
It is not FPGA output evidence. Physical-output health was therefore not
measured by this smoke; the disconnected capture must not be treated as a
platform failure.

This stable fetch signature is only a deployment smoke sample. The preserved
moving-band failure was ended by the attended reload, and no simultaneous
byte-stable source proof exists for this post-install window. It therefore
does not select the internal split and makes no root-cause claim. On a new
persistent recurrence, collect the prescribed video/source/identity window and
at least three schema-11 records before choosing schema 12 or schema 13.
