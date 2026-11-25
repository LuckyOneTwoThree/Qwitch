import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";

const FOCUSABLE_SELECTOR =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

const modalStack: symbol[] = [];
let savedBodyOverflow: string | null = null;

function getVisibleFocusables(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    (el) => el.getAttribute("aria-hidden") !== "true" && !el.closest('[aria-hidden="true"]')
  );
}

export function Modal({
  open,
  onClose,
  labelledBy,
  describedBy,
  role = "dialog",
  disableClose = false,
  flush = false,
  initialFocusRef,
  maxWidthClass = "max-w-md",
  children,
}: {
  open: boolean;
  onClose: () => void;
  labelledBy: string;
  describedBy?: string;
  role?: "dialog" | "alertdialog";
  disableClose?: boolean;
  flush?: boolean;
  initialFocusRef?: React.RefObject<HTMLElement | null>;
  maxWidthClass?: string;
  children: React.ReactNode;
}) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const previousActiveRef = useRef<HTMLElement | null>(null);
  const backdropMouseDownRef = useRef(false);
  // onClose 多为内联箭头函数：用 ref 持有，避免父组件重渲染时
  // effect 反复重建导致焦点被归还再抢走
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  const disableCloseRef = useRef(disableClose);
  disableCloseRef.current = disableClose;

  useEffect(() => {
    if (!open) return;
    const modalToken = Symbol("modal");
    modalStack.push(modalToken);

    previousActiveRef.current = document.activeElement as HTMLElement | null;

    if (modalStack.length === 1) {
      savedBodyOverflow = document.body.style.overflow;
      document.body.style.overflow = "hidden";
    }

    const focusFirst = () => {
      const root = containerRef.current;
      if (!root) return;
      if (initialFocusRef?.current && root.contains(initialFocusRef.current)) {
        initialFocusRef.current.focus();
        return;
      }
      const first = getVisibleFocusables(root)[0];
      if (first) {
        first.focus();
      } else {
        root.focus();
      }
    };
    const rafId = requestAnimationFrame(focusFirst);

    const handleKeyDown = (e: KeyboardEvent) => {
      // 仅由栈顶 Modal 响应键盘事件，防止嵌套弹窗同时关闭
      if (modalStack[modalStack.length - 1] !== modalToken) return;

      if (e.key === "Escape") {
        if (!disableCloseRef.current) {
          e.preventDefault();
          onCloseRef.current();
        }
        return;
      }

      if (e.key === "Tab") {
        const root = containerRef.current;
        if (!root) return;
        const focusables = getVisibleFocusables(root);
        if (focusables.length === 0) {
          e.preventDefault();
          return;
        }
        const first = focusables[0];
        const last = focusables[focusables.length - 1];
        if (e.shiftKey) {
          if (document.activeElement === first || !root.contains(document.activeElement)) {
            e.preventDefault();
            last.focus();
          }
        } else if (document.activeElement === last || !root.contains(document.activeElement)) {
          e.preventDefault();
          first.focus();
        }
      }
    };

    document.addEventListener("keydown", handleKeyDown);
    return () => {
      cancelAnimationFrame(rafId);
      document.removeEventListener("keydown", handleKeyDown);
      const idx = modalStack.lastIndexOf(modalToken);
      if (idx !== -1) modalStack.splice(idx, 1);
      if (modalStack.length === 0) {
        document.body.style.overflow = savedBodyOverflow ?? "";
        savedBodyOverflow = null;
      }
      if (previousActiveRef.current && previousActiveRef.current.isConnected) {
        previousActiveRef.current.focus();
      }
    };
  }, [open, initialFocusRef]);

  if (!open) return null;

  const content = (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-zinc-900/40 p-4 backdrop-blur-sm dark:bg-black/65"
      onMouseDown={(e) => {
        backdropMouseDownRef.current = e.target === e.currentTarget;
      }}
      onClick={(e) => {
        if (
          backdropMouseDownRef.current &&
          e.target === e.currentTarget &&
          !disableClose
        ) {
          onCloseRef.current();
        }
        backdropMouseDownRef.current = false;
      }}
    >
      <div
        ref={containerRef}
        role={role}
        aria-modal="true"
        aria-labelledby={labelledBy}
        aria-describedby={describedBy}
        tabIndex={-1}
        className={`w-full ${maxWidthClass} max-h-[calc(100vh-2rem)] animate-pop rounded-2xl border border-zinc-200 bg-white shadow-float outline-none dark:border-zinc-800 dark:bg-zinc-900 ${
          flush ? "flex flex-col overflow-hidden p-0" : "overflow-y-auto p-5"
        }`}
      >
        {children}
      </div>
    </div>
  );

  return typeof document !== "undefined" ? createPortal(content, document.body) : content;
}
