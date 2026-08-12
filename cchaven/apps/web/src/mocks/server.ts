import { setupServer } from "msw/node";

import { handlers } from "./handlers";

/** 测试端 mock（vitest setup 中启动，见 src/test/setup.ts）。 */
export const server = setupServer(...handlers);
