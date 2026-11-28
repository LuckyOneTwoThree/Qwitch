import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown } from "lucide-react";

export interface SelectMenuOption {
  value: string;
  label: string;
  icon?: ReactNode;
  badge?: string | number;
  disabled?: boolean;
}

export function SelectMenu({
  value,
  options,
  onChange,
  ariaLabel,
  className = "",
  minMenuWidth = 180,
}: {
  value: string;
  options: SelectMenuOption[];
  onChange: (next: string) => void;
  ariaLabel: string;
  className?: string;
  minMenuWidth?: number;
}) {
  const [open, setOpen] = useState(false);
  const [coords, setCoords] = useState<{
    top: number;
    left: number;
    minWidth: number;
  }>({
    top: 0,
    left: 0,
    minWidth: minMenuWidth,
  });
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);

  // 不回退到 options[0]：value 不在选项里时（如模型刚被移除）静默显示第一项
  // 会误导用户以为已选中它；显示原始 value 才是诚实的。
  const selected = options.find((o) => o.value === value);

  const updatePosition = useCallback(() => {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const estimatedHeight = Math.min(options.length * 32 + 12, 260);
    const menuWidth = menuRef.current?.offsetWidth ?? Math.max(rect.width, minMenuWidth);
    const menuHeight = menuRef.current?.offsetHeight ?? estimatedHeight;
    const spaceBelow = window.innerHeight - rect.bottom;
    const spaceAbove = rect.top;
    const openUp = spaceBelow < menuHeight + 8 && spaceAbove > spaceBelow;

    const top = openUp
      ? Math.max(8, rect.top - menuHeight - 4)
      : Math.min(rect.bottom + 4, Math.max(8, window.innerHeight - menuHeight - 8));
    const left = Math.min(
      Math.max(8, rect.left),
      Math.max(8, window.innerWidth - menuWidth - 8)
    );
    setCoords({
      top,
      left,
      minWidth: Math.max(rect.width, minMenuWidth),
    });
  }, [minMenuWidth, options.length]);

  useLayoutEffect(() => {
    if (!open) return;
    updatePosition();
  }, [open, updatePosition]);

  useEffect(() => {
    if (!open) return;

    const rafId = requestAnimationFrame(() => {
      const selectedEl = menuRef.current?.querySelector<HTMLButtonElement>(
        '[role="option"][aria-selected="true"]:not([disabled])'
      );
      const firstEl = menuRef.current?.querySelector<HTMLButtonElement>(
        '[role="option"]:not([disabled])'
      );
      (selectedEl ?? firstEl)?.focus();
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

    const onResizeOrScroll = () => updatePosition();

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
        menuRef.current.querySelectorAll<HTMLButtonElement>('[role="option"]:not([disabled])')
      );
      if (enabled.length === 0) return;
      const idx = enabled.indexOf(document.activeElement as HTMLButtonElement);
      if (e.key === "ArrowDown") {
        e.preventDefault();
        enabled[(idx + 1) % enabled.length]?.focus();
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        enabled[(idx - 1 + enabled.length) % enabled.length]?.focus();
      }
    };

    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    window.addEventListener("resize", onResizeOrScroll);
    window.addEventListener("scroll", onResizeOrScroll, true);
    return () => {
      cancelAnimationFrame(rafId);
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", onResizeOrScroll);
      window.removeEventListener("scroll", onResizeOrScroll, true);
    };
  }, [open, updatePosition]);

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        role="combobox"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={ariaLabel}
        onClick={() => setOpen((v) => !v)}
        className={`field-input inline-flex h-8 items-center justify-between gap-2 whitespace-nowrap py-0 pl-2.5 pr-2 text-xs ${
          open ? "border-brand-500 ring-2 ring-brand-500/15 dark:border-brand-400" : ""
        } ${className}`}
      >
        <span className="flex min-w-0 items-center gap-1.5">
          {selected?.icon && <span className="shrink-0">{selected.icon}</span>}
          <span className="truncate font-medium text-zinc-800 dark:text-zinc-100">
            {selected?.label ?? value}
          </span>
        </span>
        <ChevronDown
          className={`h-3.5 w-3.5 shrink-0 text-zinc-400 transition-transform duration-150 ${
            open ? "rotate-180 text-brand-600 dark:text-brand-400" : ""
          }`}
          aria-hidden="true"
        />
      </button>

      {open &&
        createPortal(
          <div
            ref={menuRef}
            role="listbox"
            aria-label={ariaLabel}
            style={{
              position: "fixed",
              top: coords.top,
              left: coords.left,
              minWidth: coords.minWidth,
            }}
            className="z-[90] max-h-64 overflow-y-auto rounded-xl border border-zinc-200/90 bg-white/98 p-1 shadow-[var(--shadow-float)] backdrop-blur-md dark:border-zinc-700/80 dark:bg-zinc-900/98"
          >
            {options.map((opt) => {
              const isSelected = opt.value === value;
              return (
                <button
                  key={opt.value}
                  type="button"
                  role="option"
                  aria-selected={isSelected}
                  disabled={opt.disabled}
                  onClick={() => {
                    onChange(opt.value);
                    setOpen(false);
                    triggerRef.current?.focus();
                  }}
                  className={`flex w-full items-center justify-between gap-2 rounded-lg px-2 py-1.5 text-left text-xs transition outline-none ${
                    isSelected
                      ? "bg-brand-500/12 font-semibold text-brand-900 dark:bg-brand-400/15 dark:text-brand-100"
                      : "text-zinc-700 hover:bg-zinc-100 focus:bg-zinc-100 dark:text-zinc-200 dark:hover:bg-zinc-800 dark:focus:bg-zinc-800"
                  } disabled:cursor-not-allowed disabled:opacity-45`}
                >
                  <span className="flex min-w-0 items-center gap-2">
                    {opt.icon && <span className="shrink-0">{opt.icon}</span>}
                    <span className="truncate">{opt.label}</span>
                  </span>
                  <span className="flex shrink-0 items-center gap-1.5">
                    {opt.badge !== undefined && (
                      <span className="rounded bg-black/[0.05] px-1.5 py-0.5 text-[10px] font-medium tabular-nums text-zinc-500 dark:bg-white/[0.08] dark:text-zinc-400">
                        {opt.badge}
                      </span>
                    )}
                    <Check
                      className={`h-3.5 w-3.5 shrink-0 ${
                        isSelected ? "text-brand-600 opacity-100 dark:text-brand-400" : "opacity-0"
                      }`}
                      aria-hidden="true"
                    />
                  </span>
                </button>
              );
            })}
          </div>,
          document.body
        )}
    </>
  );
}
