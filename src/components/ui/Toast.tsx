import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { AlertIcon, CheckIcon, LinkIcon } from "./icons";

type ToastKind = "success" | "error" | "info";

interface Toast {
  id: number;
  kind: ToastKind;
  text: string;
}

const ToastContext = createContext<(kind: ToastKind, text: string) => void>(
  () => {},
);

export const useToast = () => useContext(ToastContext);

const ICONS: Record<ToastKind, ReactNode> = {
  success: <CheckIcon size={15} />,
  error: <AlertIcon size={15} />,
  info: <LinkIcon size={15} />,
};

const LIFETIME_MS = 4200;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);

  const push = useCallback((kind: ToastKind, text: string) => {
    // Date.now() can collide inside the same tick; a counter cannot.
    const id = nextId();
    setToasts((prev) => [...prev, { id, kind, text }]);
    window.setTimeout(
      () => setToasts((prev) => prev.filter((t) => t.id !== id)),
      LIFETIME_MS,
    );
  }, []);

  const value = useMemo(() => push, [push]);

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className="toasts" role="status" aria-live="polite">
        {toasts.map((t) => (
          <div key={t.id} className={`toast toast--${t.kind}`}>
            <span className="toast__icon">{ICONS[t.kind]}</span>
            <span className="toast__text">{t.text}</span>
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

let counter = 0;
const nextId = () => ++counter;
