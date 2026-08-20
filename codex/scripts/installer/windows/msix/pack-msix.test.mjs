import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const templatePath = join(here, "AppxManifest.xml.template");
const packScriptPath = join(here, "Pack-Msix.ps1");
const iconRoot = join(
  here,
  "../../../../apps/codex-plus-manager/src-tauri/icons",
);

function pngSize(path) {
  const buf = readFileSync(path);
  assert.equal(buf.subarray(0, 8).toString("latin1"), "\x89PNG\r\n\x1a\n");
  assert.equal(buf.subarray(12, 16).toString("latin1"), "IHDR");
  return { width: buf.readUInt32BE(16), height: buf.readUInt32BE(20) };
}

describe("AppxManifest.xml.template", () => {
  const xml = readFileSync(templatePath, "utf8");

  it("uses issued Partner Center identity and Lumio Codex display identity", () => {
    assert.match(xml, /Name="LumioGames.LumioCodex"/);
    assert.match(xml, /Publisher="CN=BAAB68AC-52C5-48A9-B6FD-6680A662815D"/);
    assert.match(xml, /<PublisherDisplayName>Lumio Games<\/PublisherDisplayName>/);
    assert.match(xml, /<DisplayName>Lumio Codex<\/DisplayName>/);
    assert.match(xml, /DisplayName="Lumio Codex"/);
    assert.match(xml, /Description="Lumio Codex \(BestCodex\)"/);
    assert.match(xml, /Application Id="BestCodex"/);
    assert.match(xml, /Executable="lumio-codex.exe"/);
    assert.doesNotMatch(xml, /<DisplayName>BestCodex<\/DisplayName>/);
    assert.doesNotMatch(xml, /DisplayName="BestCodex"/);
  });

  it("leaves Identity.Version as a substitution token, not a live store version", () => {
    assert.match(xml, /Version="__MSIX_VERSION__"/);
    assert.match(xml, /Identity\.Name/);
    assert.match(xml, /Identity\.Publisher/);
    assert.match(xml, /PublisherDisplayName/);
  });

  it("sets Wide310x150Logo on DefaultTile when Square310x310Logo is present", () => {
    const defaultTile = xml.match(/<uap:DefaultTile\b[^/]*\/>/)?.[0] ?? "";
    assert.match(defaultTile, /Square310x310Logo="Assets\\Square310x310Logo\.png"/);
    // makeappx 80080204: Square310x310Logo requires Wide310x150Logo
    assert.match(defaultTile, /Wide310x150Logo="Assets\\Wide310x150Logo\.png"/);
  });
});

describe("Pack-Msix.ps1 contracts", () => {
  const script = readFileSync(packScriptPath, "utf8");

  it("writes the store-unsigned MSIX next to NSIS/ZIP and not into the ZIP staging dir", () => {
    assert.match(script, /LumioCodex-\$\{?PackageVersion\}?-windows-x64-store-unsigned\.msix/);
    assert.match(script, /msix-stage/);
    assert.doesNotMatch(script, /dist\\windows\\app\\AppxManifest/);
  });

  it("fails clearly when Windows Kits makeappx.exe is missing", () => {
    assert.match(script, /Windows Kits\\10\\bin\\\*\\x64\\makeappx\.exe/);
    assert.match(script, /makeappx\.exe not found/i);
  });

  it("does not fetch a store MSIX or embed Partner Center secrets", () => {
    assert.doesNotMatch(script, /store\.rg-adguard\.net/i);
    assert.doesNotMatch(script, /JasonWei512/i);
    assert.doesNotMatch(script, /SIGNPATH|PARTNER_CENTER_TOKEN|client_secret/i);
  });

  it("copies Wide310x150Logo.png with the other tile assets", () => {
    assert.match(script, /'Wide310x150Logo\.png'/);
    assert.match(script, /'Square310x310Logo\.png'/);
  });

  it("msix stage carries the sync components", () => {
    assert.match(script, /fns-agent\.exe/);
    assert.match(script, /resources\\remote\\linux-x86_64/);
  });
});

describe("MSIX tile assets", () => {
  it("ships a 310x150 Wide310x150Logo next to the square tile assets", () => {
    const logo = join(iconRoot, "Wide310x150Logo.png");
    assert.equal(existsSync(logo), true, `missing ${logo}`);
    const { width, height } = pngSize(logo);
    assert.equal(width, 310);
    assert.equal(height, 150);
  });
});
