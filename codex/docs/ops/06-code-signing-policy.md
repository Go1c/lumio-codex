# Code signing policy

This page is the public **Code signing policy** for the Lumio Codex desktop client in `LumioGames/lumio-codex` (`codex/`).

Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org).

The Authenticode publisher name is **SignPath Foundation**, not Lumio. macOS Developer ID signing and notarization are out of scope for this policy.

## What is signed

When the SignPath token is configured on the release path (`publish`, `v*` tags, or `workflow_dispatch`):

- `lumio-codex.exe`
- `lumio-codex-launcher.exe`
- `LumioCodex-<version>-windows-x64-setup.exe`

The portable zip is rebuilt from the two signed executables. Pull requests stay unsigned. `uninstall.exe` is generated on the user's machine by NSIS and is not signed.

This repository does **not** sign the official OpenAI Codex / ChatGPT application, and does not bundle it.

## Two Windows tracks (do not collapse them)

These tracks coexist. One does not replace the other.

1. **Website / GitHub Release (this page).** SignPath Authenticode on the two PE files and the NSIS setup. This is the existing CI path. Do not change that signing logic to cover MSIX.
2. **Microsoft Store.** CI packs `LumioCodex-*-windows-x64-store-unsigned.msix` with `makeappx` from the cargo staging dir. SignPath does **not** sign that MSIX. After Partner Center listing, Microsoft re-signs the package. Store Identity is still a placeholder. Full process (中文): [07-microsoft-store.md](./07-microsoft-store.md). Decision: [0011](../../../.spec/decisions/0011-windows-msix-store-scaffold.md).

Do not submit the store MSIX to SignPath. Do not put Partner Center tokens in this repository.

## Team roles

- Committers and reviewers: [LumioGames organization members](https://github.com/orgs/LumioGames/people) who can open and review pull requests on [LumioGames/lumio-codex](https://github.com/LumioGames/lumio-codex).
- Approvers: [repository owners](https://github.com/LumioGames/lumio-codex) of `LumioGames/lumio-codex`.

GitHub organization members who can merge to release branches must keep multi-factor authentication enabled.

## Privacy

This program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it.

Account, billing, and model requests the user starts after install go to `https://api.lumio.games/` as documented in the product README. Signing credentials never enter this repository.

## Apply and operate SignPath

1. Submit the project at [signpath.org/apply](https://signpath.org/apply.html) for `https://github.com/LumioGames/lumio-codex`. Describe only the AGPL `codex/` desktop client. Do not present `web/` or `cchaven/` as the signed artifact.
2. After approval, install the [SignPath GitHub App](https://github.com/apps/signpath) on `LumioGames/lumio-codex`.
3. Create a release signing policy restricted to `publish` / tags as SignPath requires for origin verification.
4. Set repository secret `SIGNPATH_API_TOKEN` and variables `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG`, `SIGNPATH_POLICY_SLUG`. Optional: bind the secret to a `windows-signing` environment.
5. Artifact configurations live in [`.signpath/artifact-configurations/`](../../../.signpath/artifact-configurations/). They cover the two PE files and the NSIS setup only — not the store MSIX.
6. Run **Internal unsigned build artifacts** with `workflow_dispatch` on `dev` or `publish` and confirm `Get-AuthenticodeSignature` is Valid on the three PE files.

If SignPath rejects the application, keep this filename and CI switch; replace only the submit step with Azure Artifact Signing or SSL.com eSigner. Do not put a PFX in GitHub Secrets — new Authenticode keys are not exportable.

## Related

- [03-release.md](./03-release.md) — channels, S3 pointer, public release gate
- [07-microsoft-store.md](./07-microsoft-store.md) — Microsoft Store flow and which track signs what
- [01-local-build.md](./01-local-build.md) — local unsigned packaging
- [0011](../../../.spec/decisions/0011-windows-msix-store-scaffold.md) — unsigned MSIX track decision
