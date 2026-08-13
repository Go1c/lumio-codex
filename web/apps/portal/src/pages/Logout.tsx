import { useEffect, useRef } from "react";
import { useSearchParams } from "react-router-dom";

import { LoadingBlock } from "@lumio/ui";

import { useAuthRedirect } from "@/lib/redirect";
import { usePortalSession } from "@/state/session";

/** 产品站的「退出」链接落在这里：清完会话再按 next 回到来处。 */
export function Logout() {
  const [params] = useSearchParams();
  const next = params.get("next");
  const session = usePortalSession();
  const redirect = useAuthRedirect(next);
  const done = useRef(false);

  useEffect(() => {
    if (done.current) return;
    done.current = true;
    void session.signOut().then(redirect);
  }, [redirect, session]);

  return (
    <div className="auth-page">
      <div className="auth-card">
        <LoadingBlock label="正在退出登录…" lines={2} />
      </div>
    </div>
  );
}
