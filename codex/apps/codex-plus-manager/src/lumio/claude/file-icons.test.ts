import assert from "node:assert/strict";
import test from "node:test";

import { fileIconKind } from "./file-icons.ts";

test("directories use dir and dirOpen", () => {
  assert.equal(fileIconKind("src", "dir"), "dir");
  assert.equal(fileIconKind("src", "dir", true), "dirOpen");
});

test("dotfiles take the last segment after a dot as the extension", () => {
  assert.equal(fileIconKind(".gitignore", "file"), "fileText");
  assert.equal(fileIconKind("src/.env", "file"), "fileText");
});

test("text, code, and config extensions map to the prototype kinds", () => {
  assert.equal(fileIconKind("README.md", "file"), "fileText");
  assert.equal(fileIconKind("src/lib.rs", "file"), "fileCode");
  assert.equal(fileIconKind("src/app.tsx", "file"), "fileCode");
  assert.equal(fileIconKind("package.json", "file"), "fileConf");
  assert.equal(fileIconKind("Cargo.lock", "file"), "fileConf");
});

test("missing or unknown extensions use the generic file icon", () => {
  assert.equal(fileIconKind("Makefile", "file"), "file");
  assert.equal(fileIconKind("notes", "file"), "file");
  assert.equal(fileIconKind("photo.png", "file"), "file");
});
