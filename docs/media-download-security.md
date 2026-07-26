# Media Download Trust Model

MiSTer MagiK deliberately keeps large screenshot packs and index sidecars
publicly downloadable over HTTP. Avoiding TLS on the MiSTer reduces CPU cost,
but HTTP provides neither confidentiality nor transport authenticity. The
runtime therefore trusts signed metadata, not the object connection.

## Trust Boundary

The release flow is:

1. A publisher uploads immutable, content-addressed pack and index objects.
2. The publisher signs the exact `manifest.json` bytes with Ed25519.
3. The publisher deploys `manifest.json` and `manifest.json.sig` together with
   the same no-cache policy.
4. Every client fetches both metadata files over HTTPS, refuses HTTP redirects,
   and verifies the signature before UTF-8 conversion or JSON parsing.
5. The verified manifest supplies each object's expected byte length and
   SHA-256 hash. Object bytes may then arrive over HTTP and are installed only
   after both values match.

This prevents an attacker on the HTTP path from substituting a different
release unless they also hold an accepted signing key. It does not make the R2
custom domain private. Anyone who knows or discovers a public object URL can
download it; signatures authenticate releases but provide no download
authorization, confidentiality, rate limiting, or hotlink protection.

## Enforced Limits

| Resource | Limit |
| --- | ---: |
| Manifest response | 256 KiB |
| Signature response | 4 KiB |
| Declared screenshot pack | 128 MiB |
| Declared index sidecar | 8 MiB |
| Object connection establishment | 10 seconds |
| Complete object transfer | 20 minutes |

Manifest and signature redirects remain HTTPS-only. Object downloads accept
HTTP or HTTPS. Curl's size limit is defense in depth; clients also bound the
stream themselves and never rely on `Content-Length`. A stream stops before
writing byte `expected + 1`.

Missing, malformed, unknown-key, or invalid signatures fail closed before
manifest parsing. Short, oversized, timed-out, or hash-mismatched objects fail
before publication. Curl and hashing children are reaped, staging files are
removed, and any previously installed pack remains in place.

## Signing-Key Custody

The production key ID is `media-prod-2026-01`. Its 32-byte signing seed is
stored as the private publisher repository's GitHub Actions secret
`MAGIK_MANIFEST_SIGNING_SEED`; only the public key and key ID are committed.
Publishing must materialize the seed into an ephemeral file with mode `0600`,
set `MAGIK_MANIFEST_SIGNING_KEY_FILE` to that file, and remove it after use.
Private key material must never be committed, uploaded to R2, or included in a
site artifact.

An enforcement release must not ship until the signed manifest and detached
signature have been deployed together. Production deployment remains an
attended, separately authorized operation.

## Two-Key Rotation

Rotate without a flag day:

1. Generate the replacement seed outside both repositories and store it as a
   new protected publisher secret.
2. Release clients that trust both the old and replacement public key IDs.
3. After that client release is established, switch the publisher to the new
   key and deploy a newly signed manifest.
4. Keep both public keys trusted through the compatibility window.
5. Release clients that remove the old public key, then destroy the retired
   seed and remove its publisher secret.

For emergency compromise, stop publishing, remove the compromised key from a
client release, and do not resume until clients trust a replacement key.
