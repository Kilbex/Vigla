import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  label: string;
  children: ReactNode;
  resetKey?: unknown;
}

interface State {
  error: Error | null;
}

export default class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error(`Vigla ${this.props.label} crashed`, error, info);
  }

  componentDidUpdate(prevProps: Props): void {
    if (Object.is(prevProps.resetKey, this.props.resetKey)) return;
    if (this.state.error) this.setState({ error: null });
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <section className="surface-error-boundary" role="alert">
        <h2>{this.props.label} unavailable</h2>
        <p>{this.state.error.message || "This surface failed to render."}</p>
        <button
          type="button"
          className="surface-error-boundary__button"
          onClick={() => this.setState({ error: null })}
        >
          Retry surface
        </button>
      </section>
    );
  }
}
