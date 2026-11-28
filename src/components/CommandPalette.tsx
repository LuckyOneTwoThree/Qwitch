import React, { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { LucideIcon } from "lucide-react";
import { CornerDownLeft, Search } from "lucide-react";
import { ToolBrandIcon } from "./ToolBrandIcon";

export interface CommandItem {
  id: string;
  group: string;
  label: string;
  /** 右侧副信息：账号水位、快捷键、数量等 */
  meta?: string;
  /** 额外的匹配词：UID、工具名、拼音首字母等 */
  keywords?: string;
  icon: LucideIcon;
  /** 账号条目展示真实品牌图标，优先于 icon */
  toolId?: string;
  run: () => void;
}

/**
 * ⌘K 命令面板。
 *
 * 之前的 ⌘K 只是把焦点塞进搜索框——那只能筛当前视图，
 * 不能「跳到某个账号」也不能「执行一个动作」。这里把两者合并：
 * 输入即搜账号，输错也没关系，下面永远有可执行的操作兜底。
 */
export function CommandPalette({
  open,
  onClose,
  items,
}: {
  open: boolean;
  onClose: () => void;
  items: CommandItem[];
}) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return items;
    return items.filter((it) => {
      const hay = `${it.label} ${it.meta ?? ""} ${it.keywords ?? ""} ${it.group}`.toLowerCase();
      return hay.includes(q);
    });
  }, [items, query]);

  // 分组保序渲染
  const grouped = useMemo(() => {
    const map = new Map<string, CommandItem[]>();
    for (const it of filtered) {
      const list = map.get(it.group) ?? [];
      list.push(it);
      map.set(it.group, list);
    }
    return [...map.entries()];
  }, [filtered]);

  useEffect(() => {
    if (!open) {
      setQuery("");
      setActive(0);
      return;
    }
    inputRef.current?.focus();
  }, [open]);

  useEffect(() => {
    setActive(0);
  }, [query]);

  // 让高亮项始终可见
  useEffect(() => {
    listRef.current
      ?.querySelector<HTMLElement>(`[data-index="${active}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [active]);

  if (!open) return null;

  const runAt = (index: number) => {
    const target = filtered[index];
    if (!target) return;
    onClose();
    // 先关闭再执行：动作可能触发路由跳转或打开抽屉，先关面板避免遮罩残留
    target.run();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((i) => (filtered.length ? (i + 1) % filtered.length : 0));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) => (filtered.length ? (i - 1 + filtered.length) % filtered.length : 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      runAt(active);
    }
  };

  let cursor = -1;

  const content = (
    <div
      className="fixed inset-0 z-[60] flex items-start justify-center bg-zinc-950/25 px-4 pt-[12vh] backdrop-blur-[2px]"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="命令面板"
        onKeyDown={onKeyDown}
        className="flex max-h-[60vh] w-full max-w-[560px] flex-col overflow-hidden rounded-xl border border-zinc-200 bg-white shadow-2xl dark:border-white/[0.1] dark:bg-[#15171c]"
      >
        <div className="flex shrink-0 items-center gap-2 border-b border-zinc-200/80 px-3.5 dark:border-white/[0.07]">
          <Search className="h-3.5 w-3.5 shrink-0 text-zinc-400" aria-hidden="true" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索账号，或输入操作…"
            aria-label="命令面板搜索"
            role="combobox"
            aria-expanded="true"
            aria-controls="command-list"
            aria-autocomplete="list"
            className="h-11 w-full bg-transparent text-[13px] text-zinc-900 outline-none placeholder:text-zinc-400 dark:text-zinc-100"
          />
          <kbd className="shrink-0 rounded border border-zinc-200 px-1 py-0.5 font-mono text-[10px] text-zinc-400 dark:border-zinc-700 dark:text-zinc-500">
            ESC
          </kbd>
        </div>

        <div ref={listRef} id="command-list" role="listbox" className="min-h-0 flex-1 overflow-y-auto py-1">
          {filtered.length === 0 ? (
            <p className="px-3.5 py-6 text-center text-[12px] text-zinc-500 dark:text-zinc-400">
              没有匹配项。试试账号名、UID 或「领取」「网关」这类动作词。
            </p>
          ) : (
            grouped.map(([group, list]) => (
              <div key={group} className="py-1">
                <div className="px-3.5 py-1 text-[10px] font-medium tracking-[0.08em] text-zinc-400 uppercase dark:text-zinc-500">
                  {group}
                </div>
                {list.map((item) => {
                  cursor += 1;
                  const index = cursor;
                  const selected = index === active;
                  const Icon = item.icon;
                  return (
                    <button
                      key={item.id}
                      type="button"
                      role="option"
                      aria-selected={selected}
                      data-index={index}
                      onMouseMove={() => setActive(index)}
                      onClick={() => runAt(index)}
                      className={`flex w-full items-center gap-2.5 px-3.5 py-1.5 text-left text-[12.5px] transition ${
                        selected
                          ? "bg-brand-500/10 text-brand-900 dark:bg-brand-400/[0.14] dark:text-brand-100"
                          : "text-zinc-700 dark:text-zinc-300"
                      }`}
                    >
                      {item.toolId ? (
                        <ToolBrandIcon toolId={item.toolId} size="xs" decorative />
                      ) : (
                        <Icon
                          className={`h-3.5 w-3.5 shrink-0 ${selected ? "text-brand-700 dark:text-brand-300" : "text-zinc-400"}`}
                          aria-hidden="true"
                        />
                      )}
                      <span className="min-w-0 flex-1 truncate">{item.label}</span>
                      {item.meta && (
                        <span className="shrink-0 font-mono text-[10.5px] text-zinc-400 dark:text-zinc-500">
                          {item.meta}
                        </span>
                      )}
                      {selected && (
                        <CornerDownLeft className="h-3 w-3 shrink-0 text-brand-600 dark:text-brand-300" aria-hidden="true" />
                      )}
                    </button>
                  );
                })}
              </div>
            ))
          )}
        </div>

        <div className="flex shrink-0 items-center gap-3 border-t border-zinc-200/80 px-3.5 py-1.5 text-[10.5px] text-zinc-400 dark:border-white/[0.07] dark:text-zinc-500">
          <span>↑↓ 选择</span>
          <span>↵ 执行</span>
          <span className="ml-auto tabular-nums">{filtered.length} 项</span>
        </div>
      </div>
    </div>
  );

  return typeof document !== "undefined" ? createPortal(content, document.body) : content;
}
