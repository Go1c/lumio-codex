import assert from "node:assert/strict";
import test from "node:test";

import {
  copyTextForKey,
  firstOpenableHttpsUrl,
  isClaudeLoginUrl,
  stitchWrappedHttpsUrls,
  terminalContextActions,
  textForClipboard,
} from "./terminal-clipboard.ts";

const WRAPPED_LOGIN = [
  "Login",
  "Browser didn't open? Use the url below to sign in (c to copy)",
  "https://claude.com/cai/oauth/authorize?code=true&client_id=abc",
  "def&state=xyz&redirect_uri=https://claude.com/done",
  "Paste code here if prompted >",
].join("\n");

test("stitches an OAuth URL that wrapped across terminal lines", () => {
  const urls = stitchWrappedHttpsUrls(WRAPPED_LOGIN);
  assert.deepEqual(urls, [
    "https://claude.com/cai/oauth/authorize?code=true&client_id=abcdef&state=xyz&redirect_uri=https://claude.com/done",
  ]);
});

test("does not glue a sentence after the URL onto the link", () => {
  const text = "https://claude.com/cai/oauth/authorize?code=true&x=1\nPaste code here if prompted >";
  assert.equal(
    firstOpenableHttpsUrl(text),
    "https://claude.com/cai/oauth/authorize?code=true&x=1",
  );
});

test("keeps two separate links apart", () => {
  const urls = stitchWrappedHttpsUrls("see https://example.com/a\nand https://example.com/b");
  assert.deepEqual(urls, ["https://example.com/a", "https://example.com/b"]);
});

test("copy prefers a stitched URL when the selection is the wrapped link", () => {
  const selected = [
    "https://claude.com/cai/oauth/authorize?code=true&client_id=abc",
    "def&state=xyz",
  ].join("\n");
  assert.equal(
    textForClipboard(selected, WRAPPED_LOGIN),
    "https://claude.com/cai/oauth/authorize?code=true&client_id=abcdef&state=xyz",
  );
});

test("copy uses the selected plain text when it is not a link", () => {
  assert.equal(textForClipboard("  hello  ", WRAPPED_LOGIN), "  hello  ");
});

test("copy falls back to the visible login URL when nothing is selected", () => {
  assert.equal(
    textForClipboard("", WRAPPED_LOGIN),
    "https://claude.com/cai/oauth/authorize?code=true&client_id=abcdef&state=xyz&redirect_uri=https://claude.com/done",
  );
});

test("right-click actions offer copy and open for a login screen", () => {
  const actions = terminalContextActions("", WRAPPED_LOGIN);
  assert.equal(actions.copyText?.startsWith("https://claude.com/cai/oauth/"), true);
  assert.equal(actions.openUrl, actions.copyText);
});

test("right-click copy uses the current selection even without a URL", () => {
  const actions = terminalContextActions("selected note", "no url here");
  assert.equal(actions.copyText, "selected note");
  assert.equal(actions.openUrl, null);
});

test("Cmd+C and Ctrl+C copy a selection instead of sending it to the server", () => {
  const selection = "https://claude.com/cai/oauth/authorize?x=1";
  assert.equal(
    copyTextForKey({
      key: "c",
      metaKey: true,
      ctrlKey: false,
      shiftKey: false,
      altKey: false,
      selection,
      visibleText: WRAPPED_LOGIN,
    }),
    selection,
  );
  assert.equal(
    copyTextForKey({
      key: "c",
      metaKey: false,
      ctrlKey: true,
      shiftKey: false,
      altKey: false,
      selection,
      visibleText: WRAPPED_LOGIN,
    }),
    selection,
  );
});

test("the Claude login shortcut c copies the local URL and does not need a selection", () => {
  assert.equal(
    copyTextForKey({
      key: "c",
      metaKey: false,
      ctrlKey: false,
      shiftKey: false,
      altKey: false,
      selection: "",
      visibleText: WRAPPED_LOGIN,
    })?.startsWith("https://claude.com/cai/oauth/"),
    true,
  );
});

test("only Claude OAuth URLs count as the login link", () => {
  assert.equal(isClaudeLoginUrl("https://claude.com/cai/oauth/authorize?x=1"), true);
  assert.equal(isClaudeLoginUrl("https://example.com/docs"), false);
});

test("plain c still goes to the server outside the login prompt", () => {
  assert.equal(
    copyTextForKey({
      key: "c",
      metaKey: false,
      ctrlKey: false,
      shiftKey: false,
      altKey: false,
      selection: "",
      visibleText: "root@host:~# ",
    }),
    null,
  );
});

test("Ctrl+C without a selection is left for the terminal unless a login URL is on screen", () => {
  assert.equal(
    copyTextForKey({
      key: "c",
      metaKey: false,
      ctrlKey: true,
      shiftKey: false,
      altKey: false,
      selection: "",
      visibleText: "root@host:~# ",
    }),
    null,
  );
});
