---
id: updating
title: Updating alc
sidebar_label: Update
sidebar_position: 7
description: Check for and install new alc releases with verified SHA-256 checksums on macOS, Linux, and Windows.
keywords:
  - alc update
  - self update cli
---

# Updating alc

Check for a new release, or update both `alc` and its bundled helper:

```sh
alc update --check
alc update
```

`alc update` selects the correct release for the current OS and CPU, verifies
the archive against the release's published SHA-256 checksum, checks the
packaged version, and then replaces both binaries.

- **Linux and macOS** update immediately.
- **Windows** stages the verified files and finishes replacement just after the
  running `alc.exe` exits; wait a moment before checking `alc --version`.

Use `alc update --force` to reinstall the current latest release.

:::note

Running sessions keep the binary they started with. Restart any open
`alc`-launched agent after updating.

:::
