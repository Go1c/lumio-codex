# Wave 0 report — split workspace panes and add session model

Branch: `feat/claude-workspace-d`  
Working directory: `codex/apps/codex-plus-manager`  
T0.1 / T0.2 / T0.1b: mechanical move (TDD exempt). T0.5: TDD.

## What was implemented

### T0.1 — split `ClaudeHome.tsx`

`ClaudeHome` is now the frame + five stage tabs + composition. Extracted:

| File | Contents |
|---|---|
| `ProjectRail.tsx` | left rail, `projectSummary`, entitlement line, 连接新服务器 |
| `TerminalPane.tsx` | xterm pane, visible-text copy helpers |
| `FileExplorer.tsx` | renamed from `FilesPane`, same logic |
| `ConflictsPane.tsx` | conflict list + color diff |
| `StatusDrawer.tsx` | `ServerStatusPane` + `SessionsPane` (still keyed by `stageTab`) |
| `StatusBar.tsx` | bottom status line |
| `SessionTabs.tsx` / `InitChecklist.tsx` / `LoginCard.tsx` | named exports returning `null`; **not rendered** |

`ClaudeWorkspace.tsx` still imports `{ ClaudeHome }` with the same props. Stage tabs and copy are unchanged.

### T0.1b — scan the views directory

Added `src/lumio/claude/read-claude-views.ts`:

- `readAllClaudeViews()` concatenates all non-test `.tsx`/`.ts` under `views/claude/`
- `readAllClaudeCss()` concatenates all `.css` in that folder

Source-scan tests now assert against the concatenation (meaning unchanged). The “exactly 6 tsx files” pin now checks that the original 6 **and** the new pane files exist.

### T0.2 — split `claude-workspace.css`

`claude-workspace.css` is a barrel (`@import` the per-pane sheets). Selectors / properties / values / media queries were cut-and-pasted, not rewritten. Combined `.lumio-claude-term, .lumio-claude-files` kept as one rule (lives in `TerminalPane.css`). Subscribe / connect / empty styles remain in the barrel. SessionTabs / InitChecklist / LoginCard CSS are comment-only placeholders.

### T0.3 — session / server types

Added `ClaudeChatSession`, CLI install + login status, `ClaudeStatusDrawerPane`, `ClaudeWorkspacePhase`, new `ClaudeState` fields (empty-record defaults), and the new `ClaudeEvent` variants. `ClaudeStageTab` / `stageTab` / `set-stage-tab` kept. `PersistableClaudeState` unchanged.

### T0.4 — reducer

`initialClaudeState()` defaults: empty records, `statusDrawer: "closed"`, `stageTab: "terminal"`. New event branches only; old branches including `sync-finished` → `stageTab: "terminal"` untouched. `close-session` always leaves at least the `nextSessionId` session (synthesizes `{ title: null, titleLocked: false, running: false }` if missing).

### T0.5 — pure modules (TDD)

`session-title.ts`, `rail-groups.ts`, `explorer-filter.ts`, `file-icons.ts` with matching `*.test.ts`. Logic aligned with `prototype.js` (`clipWidth`, rail open/collapse, `fxMatcher` / `fxGlobs` / `FX_SORTED`, `FX_TEXT`/`FX_CODE`/`FX_CONF`).

## TDD evidence for T0.5

### RED

Command (tests written; production modules not yet present):

```
cd /Users/cui/Sites/lumio-codex/codex/apps/codex-plus-manager && node --test src/lumio/claude/session-title.test.ts src/lumio/claude/rail-groups.test.ts src/lumio/claude/explorer-filter.test.ts src/lumio/claude/file-icons.test.ts src/lumio/claude/machine.test.ts
```

Output (abridged):

```
Error [ERR_MODULE_NOT_FOUND]: Cannot find module '.../explorer-filter.ts'
Error [ERR_MODULE_NOT_FOUND]: Cannot find module '.../file-icons.ts'
Error [ERR_MODULE_NOT_FOUND]: Cannot find module '.../rail-groups.ts'
Error [ERR_MODULE_NOT_FOUND]: Cannot find module '.../session-title.ts'
✖ open-session appends the session and makes it active
  TypeError: Cannot read properties of undefined (reading 'length')
✖ toggle-server-group flips collapsedHosts for that host
  AssertionError: undefined !== true
✖ set-status-drawer and set-workspace-phase write through
  AssertionError: 'closed' !== 'conflicts'
ℹ tests 45
ℹ pass 31
ℹ fail 14
```

T0.5 failed because the modules did not exist (feature missing). T0.4 new-event tests failed because the reducer still hit `default` and left state unchanged.

### GREEN

Same command after implementing the four modules and the new reducer branches:

```
ℹ tests 71
ℹ pass 71
ℹ fail 0
ℹ duration_ms 503.018459
```

## Files changed

Implementation / tests (this wave):

- `src/lumio/views/claude/ClaudeHome.tsx` (shell only)
- `src/lumio/views/claude/ProjectRail.tsx` + `.css`
- `src/lumio/views/claude/TerminalPane.tsx` + `.css`
- `src/lumio/views/claude/FileExplorer.tsx` + `.css`
- `src/lumio/views/claude/ConflictsPane.tsx` + `.css`
- `src/lumio/views/claude/StatusDrawer.tsx` + `.css`
- `src/lumio/views/claude/StatusBar.tsx` + `.css`
- `src/lumio/views/claude/SessionTabs.tsx` + `.css` (placeholder)
- `src/lumio/views/claude/InitChecklist.tsx` + `.css` (placeholder)
- `src/lumio/views/claude/LoginCard.tsx` + `.css` (placeholder)
- `src/lumio/views/claude/claude-workspace.css` (barrel)
- `src/lumio/views/claude/claude-copy.test.ts`
- `src/lumio/claude/types.ts`
- `src/lumio/claude/machine.ts`
- `src/lumio/claude/machine.test.ts`
- `src/lumio/claude/read-claude-views.ts`
- `src/lumio/claude/session-title.ts` + `.test.ts`
- `src/lumio/claude/rail-groups.ts` + `.test.ts`
- `src/lumio/claude/explorer-filter.ts` + `.test.ts`
- `src/lumio/claude/file-icons.ts` + `.test.ts`
- `src/lumio/claude/acceptance.test.ts`
- `src/lumio/claude/color-diff.test.ts`
- `src/lumio/claude/file-tree.test.ts`
- `src/lumio/claude/remote-status.test.ts`
- `src/lumio/claude/sync-status.test.ts`
- `.sdd/task-w0-report.md`

Not touched / not staged: prototypes, `codex/docs/plans/*`, `docs/ops/*`, `package.json`, `Cargo.toml`, `.gitignore`, `src-tauri/`, `ClaudeWorkspace.tsx`.

## Verification

```
cd /Users/cui/Sites/lumio-codex/codex/apps/codex-plus-manager
npm run check
npm test
```

`npm run check`:

```
> lumio-codex@1.2.46 check
> tsc --noEmit -p tsconfig.json
```

Exit 0.

`npm test`:

```
> lumio-codex@1.2.46 test
> node --test "src/**/*.test.ts"
…
ℹ tests 340
ℹ suites 5
ℹ pass 340
ℹ fail 0
ℹ cancelled 0
ℹ skipped 0
ℹ todo 0
ℹ duration_ms 1773.580542
```

Exit 0. Cargo / `vite:build` not run (out of wave scope).

## Self-review

- Stage tabs, classNames, and user-visible copy are still in place; Wave 2 owns their removal.
- Combined CSS selectors were not split (would have changed selectors). Barrel `@import` order is the brief’s order; more-specific rail rules still beat the entitlement/orders base rules.
- `ClaudeWorkspace` props and `import "./claude-workspace.css"` unchanged.
- New reducer events only; persistable snapshot still has no sessions / no passwords.
- Placeholders compile and are not mounted.
- `file-tree.ts` merge / badges were not rewritten.

## Concerns

- `isServerGroupOpen` uses the brief formula `online && !collapsed` after the single-server / active-project exceptions. An offline group cannot be expanded by toggling `collapsedHosts` alone; Wave 1 UI will need that contract if users must expand offline hosts.
- `read-claude-views.ts` lives under production `src/lumio/claude/` and uses `node:fs`. Only tests import it; do not import it from the UI bundle.
- SessionTabs / InitChecklist / LoginCard are null shells. Wave 1 must own the real UI and must not assume they are already in the tree.
- `.lumio-claude-term, .lumio-claude-files` shared block lives in `TerminalPane.css`. File explorer still gets those rules via the barrel import.
