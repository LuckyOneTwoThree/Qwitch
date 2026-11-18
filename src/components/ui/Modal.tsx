import { useEffect, useRef } from "react";

const FOCUSABLE_SELECTOR =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function Modal({
  open,
  onClose,
  labelledBy,
  disableClose = false,
  maxWidthClass = "max-w-md",
  children,
}: {
  open: boolean;
  onClose: () => void;
  labelledBy: string;
  disableClose?: boolean;
  maxWidthClass?: string;
  children: React.ReactNode;
}) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const previousActiveRef = useRef<HTMLElement | null>(null);
  // onClose 多为内联箭头函数：用 ref 持有，避免父组件重渲染时
  // effect 反复重建导致焦点被归还再抢走
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    if (!open) return;
    previousActiveRef.current = document.activeElement as HTMLElement | null;

    const prevOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";

    const focusFirst = () => {
      const root = containerRef.current;
      if (!root) return;
      const first = root.querySelector<HTMLElement>(FOCUSABLE_SELECTOR);
      if (first) {
        first.focus();
      } else {
        root.focus();
      }
    };
    const rafId = requestAnimationFrame(focusFirst);

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        if (!disableClose) {
          e.preventDefault();
          onCloseRef.current();
        }
        return;
      }

      if (e.key === "Tab") {
        const root = containerRef.current;
        if (!root) return;
        const focusables = Array.from(
          root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)
        );
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
      document.body.style.overflow = prevOverflow;
      if (previousActiveRef.current && previousActiveRef.current.isConnected) {
        previousActiveRef.current.focus();
      }
    };
  }, [open, disableClose]);

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/45 p-4 backdrop-blur-xs"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget && !disableClose) {
          onCloseRef.current();
        }
      }}
    >
      <div
        ref={containerRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelledBy}
        tabIndex={-1}
        className={`w-full ${maxWidthClass} rounded-2xl border border-zinc-200 bg-white p-5 shadow-xl outline-none dark:border-zinc-800 dark:bg-zinc-900`}
      >
        {children}
      </div>
    </div>
  );
}
