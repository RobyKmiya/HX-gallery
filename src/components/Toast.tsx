import { useEffect } from "react";
import { AlertIcon, CheckIcon, CloseIcon } from "./Icons";

export type ToastKind = "info" | "success" | "error";

export interface ToastData {
  id: number;
  message: string;
  kind: ToastKind;
}

interface ToastProps {
  toast: ToastData;
  onClose: () => void;
}

export default function Toast({ toast, onClose }: ToastProps) {
  useEffect(() => {
    const timeout = window.setTimeout(onClose, toast.kind === "error" ? 7000 : 4200);
    return () => window.clearTimeout(timeout);
  }, [toast.id, toast.kind, onClose]);

  return (
    <div className={`toast toast-${toast.kind}`} role="status">
      <span className="toast-icon">
        {toast.kind === "error" ? (
          <AlertIcon width={16} height={16} />
        ) : (
          <CheckIcon width={16} height={16} />
        )}
      </span>
      <span className="toast-message">{toast.message}</span>
      <button className="toast-close" onClick={onClose} aria-label="关闭提示">
        <CloseIcon width={14} height={14} />
      </button>
    </div>
  );
}
