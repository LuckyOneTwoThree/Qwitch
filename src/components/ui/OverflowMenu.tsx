import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { MoreHorizontal, ChevronDown, type LucideIcon } from "lucide-react";

export interface OverflowMenuItem {
  label: string;
  icon: LucideIcon;
  onSelect: () => void;
  danger?: boolean;
  disabled?: boolean;
}

export function OverflowMenu({
  items,
  label,
  display = "icon",
  disabled = false,
}: {
  items: OverflowMenuItem[];
  label: string;
  /** icon = 只有「更多」图标按钮；text = 带文字的下拉按钮（用于更显眼的动作入口） */
  display?: "icon" | "text";
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [coords, setCoords] = useState<{ top: number; left: number; placement: "top" | "bottom" }>({
    top: 0,
    left: 0,
    placement: "top",
  });
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);

  const updatePosition = useCallback(() => {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const menuWidth = menuRef.current?.offsetWidth ?? 144;
    const menuHeight = menuRef.current?.offsetHeight ?? items.length * 32 + 10;
    const spaceBelow = window.innerHeight - rect.bottom;
    const spaceAbove = rect.top;
    const placement: "top" | "bottom" =
      spaceBelow < menuHeight + 8 && spaceAbove > spaceBelow ? "top" : "bottom";

    const top =
      placement === "bottom"
        ? Math.min(rect.bottom + 4, window.innerHeight - menuHeight - 8)
        : Math.max(8, rect.top - menuHeight - 4);
    const left = Math.min(
      Math.max(8, rect.left),
      Math.max(8, window.innerWidth - menuWidth - 8)
    );
    setCoords({ top, left, placement });
  }, [items.length]);

  useLayoutEffect(() => {
    if (!open) return;
    updatePosition();
  }, [open, updatePosition]);

  useEffect(() => {
    if (!open) return;

    const rafId = requestAnimationFrame(() => {
      const first = menuRef.current?.querySelector<HTMLButtonElement>(
        '[role="menuitem"]:not([disabled])'
      );
      first?.focus();
    });

    const onDown = (e: MouseEvent) => {
      const target = e.target as Node;
      if (
        triggerRef.current?.contains(target) ||
        menuRef.current?.contains(target)
      ) {
        return;
      }
      setOpen(false);
    };

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        setOpen(false);
        triggerRef.current?.focus();
        return;
      }
      if (e.key === "Tab") {
        setOpen(false);
        return;
      }
      if (!menuRef.current) return;
      const enabled = Array.from(
        menuRef.current.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not([disabled])')
      );
      if (enabled.length === 0) return;
      const idx = enabled.indexOf(document.activeElement as HTMLButtonElement);
      if (e.key === "ArrowDown") {
        e.preventDefault();
        enabled[(idx + 1) % enabled.length].focus();
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        enabled[(idx - 1 + enabled.length) % enabled.length].focus();
      } else if (e.key === "Home") {
        e.preventDefault();
        enabled[0].focus();
      } else if (e.key === "End") {
        e.preventDefault();
        enabled[enabled.length - 1].focus();
      }
    };

    const onScrollOrResize = () => updatePosition();

    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    window.addEventListener("resize", onScrollOrResize);
    window.addEventListener("scroll", onScrollOrResize, true);
    return () => {
      cancelAnimationFrame(rafId);
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", onScrollOrResize);
      window.removeEventListener("scroll", onScrollOrResize, true);
    };
  }, [open, updatePosition]);

  const menuEl = open ? (
    <div
      ref={menuRef}
      role="menu"
      aria-label={label}
      style={{ top: coords.top, left: coords.left }}
      data-placement={coords.placement}
      className="fixed z-[90] min-w-[8.5rem] animate-pop rounded-xl border border-zinc-200 bg-white p-1 shadow-float dark:border-zinc-700 dark:bg-zinc-800"
    >
      {items.map((item) => (
        <button
          key={item.label}
          type="button"
          role="menuitem"
          disabled={item.disabled}
          onClick={() => {
            setOpen(false);
            item.onSelect();
          }}
          className={`flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs font-medium transition disabled:opacity-45 ${
            item.danger
              ? "text-rose-600 hover:bg-rose-500/10 focus-visible:bg-rose-500/10 dark:text-rose-400 dark:hover:bg-rose-500/15 dark:focus-visible:bg-rose-500/15"
              : "text-zinc-700 hover:bg-zinc-100 focus-visible:bg-zinc-100 dark:text-zinc-200 dark:hover:bg-zinc-700 dark:focus-visible:bg-zinc-700"
          }`}
        >
          <item.icon className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          {item.label}
        </button>
      ))}
    </div>
  ) : null;

  return (
    <div className="relative shrink-0">
      <button
        ref={triggerRef}
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={label}
        title={label}
        disabled={disabled}
        onClick={() => setOpen((v) => !v)}
        className={
          display === "text"
            ? "btn btn-xs btn-secondary gap-1 disabled:opacity-45"
            : "btn btn-icon btn-secondary"
        }
      >
        {display === "text" ? (
          <>
            {label}
            <ChevronDown className="h-3 w-3" aria-hidden="true" />
          </>
        ) : (
          <MoreHorizontal className="h-3.5 w-3.5" aria-hidden="true" />
        )}
      </button>
      {menuEl && (typeof document !== "undefined" ? createPortal(menuEl, document.body) : menuEl)}
    </div>
  );
}
