import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const templatePath = join(here, "AppxManifest.xml.template");
const packScriptPath = join(here, "Pack-Msix.ps1");

describe("AppxManifest.xml.template", () => {
  const xml = readFileSync(templatePath, "utf8");

  it("uses Partner Center placeholders and BestCodex display identity", () => {
    assert.match(xml, /Name="LumioGames.BestCodex"/);
    assert.match(xml, /Publisher="CN=PLACEHOLDER-PARTNER-CENTER"/);
    assert.match(xml, /<PublisherDisplayName>Lumio<\/PublisherDisplayName>/);
    assert.match(xml, /<DisplayName>BestCodex<\/DisplayName>/);
    assert.match(xml, /Executable="lumio-codex.exe"/);
  });

  it("leaves Identity.Version as a substitution token, not a live store version", () => {
    assert.match(xml, /Version="__MSIX_VERSION__"/);
    assert.match(xml, /Identity\.Name/);
    assert.match(xml, /Identity\.Publisher/);
    assert.match(xml, /PublisherDisplayName/);
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
});
