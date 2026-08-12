import { setupServer } from "msw/node";
import { handlers } from "./handlers";

/** 测试用的 mock 服务，响应结构与 handlers.ts（即真实控制面）一致。 */
export const server = setupServer(...handlers);
