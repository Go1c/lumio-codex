import { setupWorker } from "msw/browser";

import { handlers } from "./handlers";

/** 浏览器端 mock（`npm run dev` 默认启用，见 src/main.tsx）。 */
export const worker = setupWorker(...handlers);
