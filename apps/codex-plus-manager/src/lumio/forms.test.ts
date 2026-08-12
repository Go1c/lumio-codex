import assert from "node:assert/strict";
import test from "node:test";

import {
  emailSuffixError,
  formatEmailSuffixHint,
  isValidEmail,
  passwordStrength,
  registerFormError,
  sanitizeVerifyCode,
} from "./forms.ts";
import type { RegisterFormInput } from "./forms.ts";
import type { LumioServiceSettings } from "./state.ts";

const SETTINGS: LumioServiceSettings = {
  registrationEnabled: true,
  emailVerifyEnabled: true,
  emailSuffixWhitelist: ["@example.com", "@lumio.games"],
  passwordResetEnabled: true,
  agreementEnabled: true,
  agreementRevision: "v2026-03",
  agreementDocuments: [
    { id: "terms", title: "服务条款", contentMd: "" },
    { id: "usage-policy", title: "使用政策", contentMd: "" },
  ],
  defaultModel: "gpt-example",
  siteBaseUrl: "https://api.lumio.games",
};

const VALID: RegisterFormInput = {
  email: "user@example.com",
  verifyCode: "123456",
  password: "supersecret",
  confirmPassword: "supersecret",
  acceptedDocumentIds: ["terms", "usage-policy"],
};

test("email validation accepts ordinary addresses and rejects malformed ones", () => {
  assert.equal(isValidEmail("user@example.com"), true);
  assert.equal(isValidEmail("user.name+tag@sub.example.co.uk"), true);
  assert.equal(isValidEmail("user@"), false);
  assert.equal(isValidEmail("user example.com"), false);
  assert.equal(isValidEmail(""), false);
});

test("an empty whitelist allows every suffix", () => {
  assert.equal(emailSuffixError("user@anywhere.dev", []), null);
  assert.equal(formatEmailSuffixHint([]), null);
});

test("a whitelist rejects other suffixes case-insensitively", () => {
  assert.equal(emailSuffixError("user@EXAMPLE.com", ["@example.com"]), null);
  assert.equal(
    emailSuffixError("user@other.dev", ["@example.com"]),
    "AUTH_EMAIL_DOMAIN_NOT_ALLOWED",
  );
});

test("the suffix hint lists every allowed suffix", () => {
  assert.equal(
    formatEmailSuffixHint(["@example.com", "@lumio.games"]),
    "支持的邮箱：@example.com、@lumio.games",
  );
});

test("verify code input keeps at most six digits", () => {
  assert.equal(sanitizeVerifyCode("12a3b4"), "1234");
  assert.equal(sanitizeVerifyCode("1234567"), "123456");
  assert.equal(sanitizeVerifyCode("  12 34  "), "1234");
});

test("password strength grows with length and character variety", () => {
  assert.equal(passwordStrength("abc"), "weak");
  assert.equal(passwordStrength("abcdefgh"), "weak");
  assert.equal(passwordStrength("abcdefg1"), "medium");
  assert.equal(passwordStrength("Abcdefg1!"), "strong");
});

test("a complete form produces no error", () => {
  assert.equal(registerFormError(VALID, SETTINGS), null);
});

test("form validation reports the first blocking problem as a stable code", () => {
  assert.equal(
    registerFormError({ ...VALID, email: "nope" }, SETTINGS),
    "AUTH_EMAIL_DOMAIN_NOT_ALLOWED",
  );
  assert.equal(registerFormError({ ...VALID, verifyCode: "" }, SETTINGS), "AUTH_CODE_REQUIRED");
  assert.equal(registerFormError({ ...VALID, verifyCode: "123" }, SETTINGS), "AUTH_CODE_REQUIRED");
  assert.equal(
    registerFormError({ ...VALID, password: "short", confirmPassword: "short" }, SETTINGS),
    "PASSWORD_TOO_SHORT",
  );
  assert.equal(
    registerFormError({ ...VALID, confirmPassword: "different" }, SETTINGS),
    "PASSWORD_MISMATCH",
  );
  assert.equal(
    registerFormError({ ...VALID, acceptedDocumentIds: ["terms"] }, SETTINGS),
    "AGREEMENTS_NOT_ACCEPTED",
  );
});

test("verification code is not required when the server does not enforce it", () => {
  const relaxed = { ...SETTINGS, emailVerifyEnabled: false };
  assert.equal(registerFormError({ ...VALID, verifyCode: "" }, relaxed), null);
});

test("agreements are not required when the server disables them", () => {
  const relaxed = { ...SETTINGS, agreementEnabled: false };
  assert.equal(registerFormError({ ...VALID, acceptedDocumentIds: [] }, relaxed), null);
});
