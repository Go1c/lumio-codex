import { render } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { App } from "../App";
import { mockState } from "../mocks/data";

/** 直接置为完整会话（已登录 + 已过两步验证），用于业务页面的用例。 */
export function signIn() {
  mockState.session = { loggedIn: true, mfaPassed: true, totpEnabled: true };
}

/**
 * 以指定角色登录。support 是只读角色：用户详情（明文邮箱）、禁用解禁、退款、
 * 改运营配置、导出 CSV 一律 403，MSW handlers 与后端矩阵同步。
 */
export function signInAs(role: "owner" | "ops" | "support") {
  signIn();
  mockState.admin = { ...mockState.admin, role };
}

/** 半会话：登录过但没过两步验证，访问业务接口会被后端 401 mfa_required 拦下。 */
export function signInHalfSession() {
  mockState.session = { loggedIn: true, mfaPassed: false, totpEnabled: true };
}

export function renderApp(initialPath = "/") {
  const user = userEvent.setup();
  const view = render(
    <MemoryRouter initialEntries={[initialPath]}>
      <App />
    </MemoryRouter>,
  );
  return { user, ...view };
}
