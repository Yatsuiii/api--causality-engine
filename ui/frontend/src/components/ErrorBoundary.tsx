import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  children: ReactNode;
  fallback?: ReactNode;
}

interface State {
  error: Error | null;
}

export default class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("[ErrorBoundary]", error, info.componentStack);
  }

  render() {
    if (this.state.error) {
      if (this.props.fallback) return this.props.fallback;

      return (
        <div className="flex-1 flex items-center justify-center p-8">
          <div className="max-w-md w-full">
            <h2 className="text-lg font-semibold text-error mb-2">
              Something went wrong
            </h2>
            <pre className="text-xs text-text-secondary bg-bg-surface rounded-lg p-4 overflow-auto max-h-48 whitespace-pre-wrap">
              {this.state.error.message}
            </pre>
            <button
              className="mt-4 px-4 py-2 bg-bg-surface hover:bg-bg-elevated rounded-lg text-sm text-text-primary transition-colors"
              onClick={() => this.setState({ error: null })}
            >
              Try again
            </button>
          </div>
        </div>
      );
    }

    return this.props.children;
  }
}
