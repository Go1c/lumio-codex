import { setupWorker } from "msw/browser";
import { handlers } from "./handlers";

export const worker = setupWorker(...handlers);

/** 开发期启动 mock；未被 handler 匹配的请求原样放行。 */
export function startMockWorker() {
  return worker.start({ onUnhandledRequest: "bypass", quiet: true });
}
