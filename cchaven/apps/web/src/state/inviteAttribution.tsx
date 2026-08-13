import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";

import { getCurrentInvite } from "@/api/endpoints";
import type { InviteAttribution } from "@/api/types";

/**
 * 邀请归因的共享状态，唯一判据是 `GET /api/v1/invites/current`。
 *
 * 归因载体 `cch_ref` 是 HttpOnly cookie，前端读不到，也不能自己缓存一份展示副本：
 * 那份副本不随 cookie 过期、也不随邀请码停用而失效，会让首页承诺「首月免费」而注册后拿不到。
 *
 * 请求策略：整个页面会话内只发一次，多个组件共用同一份结果，**不轮询**（接口无限频）。
 * 未确定归因前一律为 null，调用方不渲染横幅；请求失败同样保持 null，静默降级不弹错误条。
 */

interface InviteAttributionStore {
  attribution: InviteAttribution | null;
  load: () => void;
  seed: (next: InviteAttribution) => void;
}

const InviteAttributionContext = createContext<InviteAttributionStore | null>(null);

export function InviteAttributionProvider({ children }: { children: ReactNode }) {
  const [attribution, setAttribution] = useState<InviteAttribution | null>(null);
  const requested = useRef(false);

  const load = useCallback(() => {
    if (requested.current) return;
    requested.current = true;

    getCurrentInvite()
      .then(setAttribution)
      .catch(() => {
        /* 横幅是锦上添花的营销元素，挂了不该打断用户注册 */
      });
  }, []);

  const seed = useCallback((next: InviteAttribution) => {
    requested.current = true;
    setAttribution(next);
  }, []);

  return (
    <InviteAttributionContext.Provider value={{ attribution, load, seed }}>
      {children}
    </InviteAttributionContext.Provider>
  );
}

function useStore(): InviteAttributionStore {
  const store = useContext(InviteAttributionContext);
  if (!store) throw new Error("邀请归因相关 hook 必须在 InviteAttributionProvider 内使用");
  return store;
}

/**
 * 读取当前归因。首个消费者挂载时触发唯一的一次请求；
 * 返回 null 表示「尚未确定」（加载中或请求失败），调用方此时不应渲染邀请横幅。
 */
export function useInviteAttribution(): InviteAttribution | null {
  const store = useStore();
  const { load } = store;

  useEffect(() => load(), [load]);

  return store.attribution;
}

/**
 * 落地页 `/i/{code}` 刚从服务端拿到同一套 valid / inviter / trial_days 口径，
 * 就地写入即可让同一次会话内的注册页横幅立刻正确，省掉一次重复查询。
 */
export function useSeedInviteAttribution(): (next: InviteAttribution) => void {
  return useStore().seed;
}
