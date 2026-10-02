import type { ReactNode } from "react";
import "./signin.css";

interface SignInScreenProps {
    mark: ReactNode;
    title: string;
    lede?: ReactNode;
    foot?: ReactNode;
    children: ReactNode;
}

/** One column in the middle of the pane: the service's mark, what signing in gives, one button, and the quieter ways round below. */
export function SignInScreen({ mark, title, lede, foot, children }: SignInScreenProps) {
    return (
        <div className="signin-screen">
            <div className="signin">
                <span className="signin-mark">{mark}</span>
                <h2 className="signin-title">{title}</h2>
                {lede && <p className="signin-lede">{lede}</p>}
                {children}
                {foot && <p className="signin-foot">{foot}</p>}
            </div>
        </div>
    );
}

/** Stands in for the main button while the person finishes signing in somewhere else. */
export function SignInWaiting({ children, onCancel }: { children: ReactNode; onCancel: () => void }) {
    return (
        <div className="signin-waiting" role="status">
            <span className="signin-spinner" aria-hidden="true" />
            {children}
            <button type="button" className="signin-link" onClick={onCancel}>
                Cancel
            </button>
        </div>
    );
}
