import { createContext, useContext, type ReactNode } from "react";

import { getPublicConfig } from "@/api/endpoints";
import type { PublicConfig } from "@/api/types";
import { useResource, type Resource } from "@/hooks/useResource";

/**
 * `GET /api/v1/config/public` 的全局缓存。
 * 价格、邀请奖励天数、试用时长、下载版本一律从这里读，前端不写死（4.2 / 4.3 / 5.6 / 6.5）。
 */
const PublicConfigContext = createContext<Resource<PublicConfig> | null>(null);

export function PublicConfigProvider({ children }: { children: ReactNode }) {
  const resource = useResource<PublicConfig>((signal) => getPublicConfig(signal), []);
  return <PublicConfigContext.Provider value={resource}>{children}</PublicConfigContext.Provider>;
}

export function usePublicConfig(): Resource<PublicConfig> {
  const value = useContext(PublicConfigContext);
  if (!value) throw new Error("usePublicConfig 必须在 PublicConfigProvider 内使用");
  return value;
}

/** 邀请奖励天数，reward_days 为 0（或配置未加载）时按「关闭奖励」处理。 */
export function useInviteRewardDays(): number {
  const { data } = usePublicConfig();
  return data?.invite.reward_days ?? 0;
}
