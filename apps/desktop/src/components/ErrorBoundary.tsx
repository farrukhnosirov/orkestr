/**
 * Contains a render/commit error to one widget so it cannot unmount the
 * whole window (e.g. one HUD slot blanking the HUD). Shows `fallback`
 * instead of the broken subtree and logs the error once.
 */
import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  /** What to render instead of the failed subtree. */
  fallback: ReactNode;
  /** Short name for the log line. */
  name?: string;
  children?: ReactNode;
}

interface State {
  failed: boolean;
}

export class ErrorBoundary extends Component<Props, State> {
  override state: State = { failed: false };

  static getDerivedStateFromError(): State {
    return { failed: true };
  }

  override componentDidCatch(error: unknown, info: ErrorInfo): void {
    console.error(`${this.props.name ?? "widget"} failed`, error, info.componentStack);
  }

  override render(): ReactNode {
    return this.state.failed ? this.props.fallback : this.props.children;
  }
}
