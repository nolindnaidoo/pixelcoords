# winget manifests

Source of truth for what gets submitted to
[microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs). Kept
here so a submission is reviewable in this repo before it becomes a PR
against Microsoft's.

## Submitting a version

```powershell
winget install wingetcreate
wingetcreate submit --token <GITHUB_TOKEN> packaging\winget\<version>
```

The token needs to **fork microsoft/winget-pkgs**, so a fine-grained PAT
scoped to your own repositories will not do it — this one needs a classic
token with `public_repo`.

Microsoft's validation installs the package in a sandbox VM. Expect hours
to days.

## Why a zip and not an installer

The release ships a self-contained `.exe` in a zip; there is no installer
to run. `InstallerType: zip` with `NestedInstallerType: portable` tells
winget to extract it and register a PATH alias, which is the honest
description of what this is.

## When bumping

Three things move together, and the checksum is the one that bites:

- `PackageVersion` in all three files
- `InstallerUrl` and `InstallerSha256`
- `RelativeFilePath` — it carries the version in the directory name

`shasum -a 256 <zip>` and uppercase it; winget wants uppercase hex.
