import type { ClaudeProject } from "./types.ts";

export function groupProjectsByHost(
  projects: ClaudeProject[],
): { host: string; user: string; projects: ClaudeProject[] }[] {
  const groups: { host: string; user: string; projects: ClaudeProject[] }[] = [];
  const indexByHost = new Map<string, number>();
  for (const project of projects) {
    const at = indexByHost.get(project.host);
    if (at === undefined) {
      indexByHost.set(project.host, groups.length);
      groups.push({ host: project.host, user: project.user, projects: [project] });
    } else {
      groups[at].projects.push(project);
    }
  }
  return groups;
}

export function isServerGroupOpen(input: {
  host: string;
  serverCount: number;
  online: boolean;
  holdsActiveProject: boolean;
  /** User override: `undefined` = default, `true` = force closed, `false` = force open. */
  collapsed?: boolean;
}): boolean {
  if (input.serverCount === 1 || input.holdsActiveProject) return true;
  if (input.collapsed === true) return false;
  if (input.collapsed === false) return true;
  return input.online;
}

export function shouldShowServerShell(serverCount: number): boolean {
  return serverCount > 1;
}
