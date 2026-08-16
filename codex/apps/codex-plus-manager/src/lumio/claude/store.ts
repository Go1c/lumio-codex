import { initialClaudeState, persistableClaudeState, reduceClaudeState } from "./machine.ts";
import type { ClaudeEvent, ClaudeProject, ClaudeState, PersistableClaudeState } from "./types.ts";

export const CLAUDE_STORE_KEY = "bestcodex.claude.v1";

const listeners = new Set<() => void>();
const secrets = new Map<string, string>();

let state: ClaudeState = hydrateFromStorage();

function storage(): Storage | null {
  try {
    if (typeof localStorage === "undefined") return null;
    return localStorage;
  } catch {
    return null;
  }
}

function hydrateFromStorage(): ClaudeState {
  const raw = storage()?.getItem(CLAUDE_STORE_KEY);
  if (!raw) return initialClaudeState();
  try {
    const parsed = JSON.parse(raw) as PersistableClaudeState;
    if (!parsed || typeof parsed !== "object") return initialClaudeState();
    return reduceClaudeState(initialClaudeState(), {
      type: "projects-hydrated",
      projects: Array.isArray(parsed.projects) ? parsed.projects.map(normalizeProject) : [],
      activeProjectId: parsed.activeProjectId ?? null,
      entitlement: parsed.entitlement,
    });
  } catch {
    return initialClaudeState();
  }
}

function normalizeProject(project: ClaudeProject): ClaudeProject {
  return {
    ...project,
    hostAlias: project.hostAlias ?? null,
  };
}

function persist(next: ClaudeState): void {
  const box = storage();
  if (box === null) return;
  try {
    box.setItem(CLAUDE_STORE_KEY, JSON.stringify(persistableClaudeState(next)));
  } catch {
    // Quota or private mode: keep the in-memory singleton only.
  }
}

function emit(): void {
  for (const listener of listeners) listener();
}

export function getClaudeState(): ClaudeState {
  return state;
}

export function subscribeClaudeStore(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function dispatchClaude(event: ClaudeEvent): ClaudeState {
  const next = reduceClaudeState(state, event);
  if (next === state) return state;
  state = next;
  persist(next);
  emit();
  return state;
}

export function resetClaudeStore(next: ClaudeState = initialClaudeState()): void {
  state = next;
  secrets.clear();
  try {
    storage()?.removeItem(CLAUDE_STORE_KEY);
  } catch {
    /* ignore private-mode storage */
  }
  emit();
}

export function setDraftPassword(password: string): void {
  if (password === "") secrets.delete("draft");
  else secrets.set("draft", password);
}

export function draftPassword(): string {
  return secrets.get("draft") ?? "";
}

export function rememberProjectPassword(projectId: string, password: string): void {
  if (password === "") secrets.delete(projectId);
  else secrets.set(projectId, password);
}

export function projectPassword(projectId: string): string | undefined {
  return secrets.get(projectId);
}

export function takeDraftPassword(): string {
  const value = draftPassword();
  secrets.delete("draft");
  return value;
}
