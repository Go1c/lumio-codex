import { render, type RenderResult } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactElement } from "react";
import type { Api } from "../lib/api";
import { MockApi, type MockOptions } from "../lib/mockApi";
import { ApiProvider } from "../state/ApiProvider";
import { ToastProvider } from "../state/ToastProvider";

export interface Harness extends RenderResult {
  api: MockApi;
  user: ReturnType<typeof userEvent.setup>;
}

/** Render a component with the provider stack and a fresh mock backend. */
export function renderWithProviders(
  ui: ReactElement,
  options: MockOptions | { api: Api } = {},
): Harness {
  const api = "api" in options ? (options.api as MockApi) : new MockApi(options);
  // No inter-keystroke delay: these suites type a lot and assert on results,
  // not on typing cadence.
  const user = userEvent.setup({ delay: null });
  const result = render(
    <ApiProvider api={api}>
      <ToastProvider>{ui}</ToastProvider>
    </ApiProvider>,
  );
  return { ...result, api, user };
}
