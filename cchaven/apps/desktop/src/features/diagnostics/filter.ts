import type { DiagnosticEvent, TimelineFilter } from "./types.ts";

/**
 * Pure client-side timeline filter. project scoping is enforced by the
 * DiagnosticsClient before events reach the UI.
 */
export function filterEvents(
  events: readonly DiagnosticEvent[],
  filter: TimelineFilter,
): DiagnosticEvent[] {
  const level = filter.level?.trim() || "";
  const component = filter.component?.trim() || "";
  const eventName = filter.eventName?.trim() || "";
  const runId = filter.runId?.trim() || "";

  return events.filter((event) => {
    if (level && event.level !== level) return false;
    if (component && event.component !== component) return false;
    if (eventName && event.eventName !== eventName) return false;
    if (runId && event.runId !== runId) return false;
    return true;
  });
}
