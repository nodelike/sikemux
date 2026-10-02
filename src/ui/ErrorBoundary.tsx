import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
    children: ReactNode;
    label?: string;
}

interface State {
    error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
    state: State = { error: null };

    static getDerivedStateFromError(error: Error): State {
        return { error };
    }

    componentDidCatch(error: Error, info: ErrorInfo): void {
        console.error(`Sikemux ${this.props.label ?? "view"} crashed`, error, info.componentStack);
    }

    render(): ReactNode {
        if (!this.state.error) return this.props.children;
        return (
            <div className="pane-error" role="alert">
                <strong>{this.props.label ?? "This view"} crashed.</strong>
                <span>{this.state.error.message}</span>
                <button type="button" onClick={() => this.setState({ error: null })}>
                    retry
                </button>
            </div>
        );
    }
}
