import { render, type RenderOptions } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactElement } from "react";
import { MemoryRouter } from "react-router-dom";

import { App } from "@/App";
import { ToastProvider } from "@/components/Toast";
import { LangProvider } from "@/i18n";
import { InviteAttributionProvider } from "@/state/inviteAttribution";
import { PublicConfigProvider } from "@/state/publicConfig";
import { SessionProvider } from "@/state/session";

/** 渲染整个应用并落在指定路由上，路由跳转与真实运行一致。 */
export function renderApp(route = "/") {
  const view = render(
    <MemoryRouter initialEntries={[route]}>
      <App />
    </MemoryRouter>,
  );
  return { ...view, user: userEvent.setup() };
}

/** 渲染单个组件 / 分区，套上应用级 Provider。 */
export function renderWithProviders(
  ui: ReactElement,
  { route = "/", ...options }: Omit<RenderOptions, "wrapper"> & { route?: string } = {},
) {
  const view = render(
    <MemoryRouter initialEntries={[route]}>
      <LangProvider>
        <SessionProvider>
          <PublicConfigProvider>
            <InviteAttributionProvider>
              <ToastProvider>{ui}</ToastProvider>
            </InviteAttributionProvider>
          </PublicConfigProvider>
        </SessionProvider>
      </LangProvider>
    </MemoryRouter>,
    options,
  );
  return { ...view, user: userEvent.setup() };
}
