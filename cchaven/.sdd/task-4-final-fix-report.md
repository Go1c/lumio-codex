# Task 4 final EOL fix report

## RED

Added `fixture_bytes_are_marked_binary_for_cross_platform_checkout` to
`crates/fns-protocol/tests/workspace_shape.rs`, before adding any attributes.

`RUSTUP_TOOLCHAIN=stable cargo test --locked -p fns-protocol fixture_bytes_are_marked_binary_for_cross_platform_checkout`
failed as intended because `.gitattributes` was absent:

```text
fixture attributes must exist: Os { code: 2, kind: NotFound, message: "No such file or directory" }
```

The repository-pinned Rust 1.94.0 toolchain could not be refreshed because
`static.rust-lang.org` DNS lookup failed; the installed stable toolchain was
used for the Rust verification commands.

## GREEN

Added the sole root attribute rule:

```text
crates/fns-protocol/tests/fixtures/** -text
```

The same focused test then passed. `git check-attr text --
crates/fns-protocol/tests/fixtures/workspace-sync-v2/manifest.json` returned:

```text
crates/fns-protocol/tests/fixtures/workspace-sync-v2/manifest.json: text: unset
```

## `core.autocrlf=true` checkout evidence

Created isolated temporary clone `/tmp/fns-eol-lock.UhX0rI`, set
`core.autocrlf=true`, and force-checked out `HEAD`. For every fixture below,
the SHA-256 of the checkout file equaled the SHA-256 of
`git show HEAD:<path>`:

```text
cf7459afdcfa1c15094ef7c73ab7bc95ff23bb353013dbf32ea24e1bd34ac0b2 binary/header-vectors.json
d166aabf61c48cb6cd578eef07606a59661a261e550340bf4012b1ce170582c9 invalid/hashes.jsonl
2355070c84bd5fad4a88471428345ab53f4195a0ce09289a187a1ab169c846ae invalid/paths.jsonl
f31b2410f77e0df8ccdc7a95346704fd440ae3d0e1b7cc5de72164bf988478fb invalid/revisions.jsonl
db4dbc5466ce4f01ef3fe81b96fe73a7dfb24e900936d850637667ea5095d2ff manifest.json
bbe6d00abb3e9e426c608f36af6cd83d7f4c7ef97d90c4f969eea72f1403385d valid/control-frames.jsonl
d67bc4bfde9e3122c41897215b64f5cc96acfbb301f69d2e6095c22c0dc7b2fd valid/error-envelopes.jsonl
```

Verified fixture count: 7.

## Verification

- `RUSTUP_TOOLCHAIN=stable cargo test --locked -p fns-protocol --test workspace_shape` — 6 passed.
- `RUSTUP_TOOLCHAIN=stable cargo test --locked -p fns-protocol` — all protocol tests passed.
- `RUSTUP_TOOLCHAIN=stable cargo test --locked --workspace` — all workspace tests passed.
- `RUSTUP_TOOLCHAIN=stable cargo fmt --all -- --check` — passed.
- `RUSTUP_TOOLCHAIN=stable cargo clippy --locked --workspace --all-targets -- -D warnings` — passed.
- `git diff --check` — passed.
