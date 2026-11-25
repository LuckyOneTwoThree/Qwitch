import React from "react";

export class ErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    if (this.state.error) {
      return (
        <div className="flex min-h-screen flex-col items-center justify-center gap-3 bg-zinc-50 p-6 text-center dark:bg-zinc-950">
          <div className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
            Qwitch 界面遇到异常
          </div>
          <p className="max-w-md text-xs leading-relaxed text-zinc-600 dark:text-zinc-400">
            {String(this.state.error?.message ?? this.state.error)}
          </p>
          <button
            type="button"
            onClick={() => window.location.reload()}
            className="btn btn-md btn-primary"
          >
            重新加载
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}