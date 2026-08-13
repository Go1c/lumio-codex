import { createContext, useCallback, useContext, useState, ReactNode } from "react";
import { Link } from "react-router-dom";

/* Demo-wide state: error-state switch, mock auth, mock projects, toasts. */

export interface Project {
  id: string;
  name: string;
  host: string;
  remoteRoot: string;
  localRoot: string;
  status: "synced" | "syncing" | "conflicts" | "offline";
}

interface Toast {
  id: number;
  text: string;
  actionLabel?: string;
  onAction?: () => void;
}

interface DemoState {
  showErrors: boolean;
  setShowErrors: (v: boolean) => void;
  authed: boolean;
  setAuthed: (v: boolean) => void;
  email: string;
  setEmail: (v: string) => void;
  projects: Project[];
  setProjects: (p: Project[]) => void;
  invited: boolean;
  setInvited: (v: boolean) => void;
  toast: (text: string, actionLabel?: string, onAction?: () => void) => void;
}

const Ctx = createContext<DemoState>(null!);
export const useDemo = () => useContext(Ctx);

export function DemoProvider({ children }: { children: ReactNode }) {
  const [showErrors, setShowErrors] = useState(false);
  const [authed, setAuthed] = useState(false);
  const [email, setEmail] = useState("mary@example.com");
  const [projects, setProjects] = useState<Project[]>([]);
  const [invited, setInvited] = useState(false);
  const [toasts, setToasts] = useState<Toast[]>([]);

  const toast = useCallback((text: string, actionLabel?: string, onAction?: () => void) => {
    const id = Date.now() + Math.random();
    setToasts((t) => [...t, { id, text, actionLabel, onAction }]);
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), actionLabel ? 10000 : 4000);
  }, []);

  return (
    <Ctx.Provider
      value={{ showErrors, setShowErrors, authed, setAuthed, email, setEmail, projects, setProjects, invited, setInvited, toast }}
    >
      {children}
      <div className="toast-wrap">
        {toasts.map((t) => (
          <div key={t.id} className="toast">
            {t.text}
            {t.actionLabel && (
              <button
                onClick={() => {
                  t.onAction?.();
                  setToasts((x) => x.filter((y) => y.id !== t.id));
                }}
              >
                {t.actionLabel}
              </button>
            )}
          </div>
        ))}
      </div>
      <DemoBar />
    </Ctx.Provider>
  );
}

function DemoBar() {
  const { showErrors, setShowErrors, setAuthed, setProjects } = useDemo();
  return (
    <div className="demo-bar">
      <strong>原型演示</strong>
      <label>
        <input type="checkbox" checked={showErrors} onChange={(e) => setShowErrors(e.target.checked)} />
        显示错误状态
      </label>
      <span className="divider" />
      <Link to="/">官网</Link>
      <Link to="/app">APP</Link>
      <Link to="/admin">后台</Link>
      <span className="divider" />
      <Link
        to="/app"
        onClick={() => {
          setAuthed(true);
          setProjects([
            {
              id: "demo-1",
              name: "my-project",
              host: "root@43.156.20.8",
              remoteRoot: "/root/cchaven/my-project",
              localRoot: "/Users/mary/CCHaven/my-project",
              status: "synced",
            },
          ]);
        }}
      >
        载入演示数据
      </Link>
    </div>
  );
}

export const DEMO_CODE = "123456";
export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
