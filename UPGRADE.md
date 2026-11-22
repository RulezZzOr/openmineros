# Update Bundles

Build `0.1.0` implements update bundle **verification** in the
`openmineros-updater` crate. Writing to flash is not implemented yet.

## Bundle layout

A bundle is a flat directory:

```text
manifest.json     declarative description of the release
manifest.sig      hex-encoded Ed25519 signature over the manifest bytes
kernel.img        artifacts listed in the manifest
rootfs.img
```

## Manifest

```json
{
  "schema": 1,
  "name": "openmineros",
  "version": "0.2.0",
  "board": "xilinx",
  "min_upgrade_from": "0.1.0",
  "artifacts": [
    {
      "role": "rootfs",
      "path": "rootfs.img",
      "size": 16232605,
      "sha256": "…"
    }
  ]
}
```

- `board` is one of `xilinx`, `beagle-bone`, `amlogic`, `cvitek`.
- `role` is one of `kernel`, `device_tree`, `rootfs`, `rescue`. Each role may
  appear once.
- `min_upgrade_from` is optional.
- Unknown fields are rejected.

## Verification order

1. Read `manifest.json` and `manifest.sig` (regular files, at most 64 KiB).
2. Verify the signature against the release keys baked into the running
   image. Nothing is parsed before this succeeds.
3. Parse and validate the manifest.
4. Compare `board` with the running firmware.
5. Refuse downgrades unless policy allows them; enforce `min_upgrade_from`.
6. For every artifact: must be a regular file (no symlinks), size and SHA-256
   must match.
7. Refuse the bundle if it contains any file the manifest does not list.

## What a bundle can never do

- Run code during installation. There is no install script.
- Bring its own signing key.
- Reference files outside the bundle directory.
