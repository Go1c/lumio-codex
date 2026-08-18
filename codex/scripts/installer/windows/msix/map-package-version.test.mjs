import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { mapPackageVersionToMsix } from "./map-package-version.mjs";

describe("mapPackageVersionToMsix", () => {
  it("maps 1.2.46-internal-38 to 1.2.46.38", () => {
    assert.equal(mapPackageVersionToMsix("1.2.46-internal-38"), "1.2.46.38");
  });

  it("pads a three-part version with .0 when the fourth part cannot be mapped", () => {
    assert.equal(mapPackageVersionToMsix("1.2.46"), "1.2.46.0");
    assert.equal(mapPackageVersionToMsix("1.2.46-internal"), "1.2.46.0");
  });

  it("keeps an already-numeric four-part version", () => {
    assert.equal(mapPackageVersionToMsix("1.2.46.7"), "1.2.46.7");
  });

  it("rejects versions that do not contain x.y.z", () => {
    assert.throws(() => mapPackageVersionToMsix("internal-38"), /x\.y\.z/);
    assert.throws(() => mapPackageVersionToMsix("1.2"), /x\.y\.z/);
    assert.throws(() => mapPackageVersionToMsix(""), /x\.y\.z/);
  });

  it("rejects a part outside 0-65535", () => {
    assert.throws(() => mapPackageVersionToMsix("1.2.46-internal-70000"), /65535/);
  });
});
