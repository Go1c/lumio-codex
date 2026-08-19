import { formatClaudeEntitlementLine } from "../../claude/copy.ts";
import type { ClaudeEntitlement } from "../../claude/types.ts";

export function ClaudeEntitlementLine({ entitlement }: { entitlement: ClaudeEntitlement }) {
  const expiring = entitlement.expiringSoon === true || (entitlement.daysLeft ?? 99) <= 3;
  const showExpiryWarning =
    expiring && (entitlement.status === "active" || entitlement.status === "trialing");
  return (
    <p className={`lumio-claude-entitlement${showExpiryWarning ? " is-expiring" : ""}`}>
      {formatClaudeEntitlementLine(entitlement)}
      {showExpiryWarning ? <span> 即将到期，请及时续期。</span> : null}
    </p>
  );
}
