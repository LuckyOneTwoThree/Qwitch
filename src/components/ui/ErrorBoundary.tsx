import React from "react";
import { AlertTriangle, Check, Copy, RotateCcw } from "lucide-react";

export class ErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null; componentStack: string | null; copied: boolean }
> {
  state = {
    error: null as Error | null,
    componentStack: null as string | null,
    copied: false,
  };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error("[Qwitch ErrorBoundary]", error, info.componentStack);
    this.setState({ componentStack: info.componentStack ?? null });
  }

  private handleCopyDetails = () => {
    const err = this.state.error;
    if (!err) return;
    const text = [
      `Error: ${err.message || String(err)}`,
      err.stack ? `\nStack:\n${err.stack}` : "",
      this.state.componentStack ? `\nComponentStack:${this.state.componentStack}` : "",
    ]
      .filter(Boolean)
      .join("\n");
    void navigator.clipboard
      ?.writeText(text)
      .then(() => {
        this.setState({ copied: true });
        setTimeout(() => this.setState({ copied: false }), 2000);
      })
      .catch(() => {});
  };

  render() {
    if (this.state.error) {
      return (
        <div className="flex h-full min-h-[260px] w-full items-center justify-center bg-zinc-50 p-4 dark:bg-zinc-950">
          <div className="surface-card w-full max-w-md p-5 text-center">
            <div className="mx-auto mb-3 flex h-10 w-10 items-center justify-center rounded-xl bg-rose-500/12 text-rose-600 dark:text-rose-400">
              <AlertTriangle className="h-5 w-5" aria-hidden="true" />
            </div>
            <div className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
              Qwitch 界面遇到异常
            </div>
            <p className="mt-1.5 break-words text-xs leading-relaxed text-zinc-600 dark:text-zinc-400">
              {String(this.state.error?.message ?? this.state.error)}
            </p>
            <div className="mt-4 flex flex-wrap items-center justify-center gap-2">
              <button
                type="button"
                onClick={() => this.setState({ error: null, componentStack: null })}
                className="btn btn-md btn-secondary"
              >
                <RotateCcw className="h-3.5 w-3.5" aria-hidden="true" />
                尝试恢复
              </button>
              <button
                type="button"
                onClick={() => window.location.reload()}
                className="btn btn-md btn-primary"
              >
                重新加载
              </button>
              <button
                type="button"
                onClick={this.handleCopyDetails}
                className="btn btn-md btn-ghost"
              >
                {this.state.copied ? (
                  <Check className="h-3.5 w-3.5 text-brand-600" aria-hidden="true" />
                ) : (
                  <Copy className="h-3.5 w-3.5" aria-hidden="true" />
                )}
                {this.state.copied ? "已复制详情" : "复制错误详情"}
              </button>
            </div>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
