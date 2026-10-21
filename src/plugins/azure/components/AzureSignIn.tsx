import { useState } from "react";
import { copyWithNotice, openUrl, swallow } from "../../../plugin-api/host";
import { EmptyState, SignInError } from "../../../plugin-api/ui";
import { errorText } from "../../../plugin-api/format";
import { azureApi, type AzureStatus } from "../api";
import { AzureMark } from "./AzureMark";

const INSTALL = "https://learn.microsoft.com/cli/azure/install-azure-cli-macos";

/** Signing in happens in the Azure CLI, which opens the browser and keeps the sign-in itself. */
export function AzureSignIn({ status, onSignedIn }: { status: AzureStatus | undefined; onSignedIn: () => void }) {
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const missing = status?.category === "cli-missing";

    const signIn = async () => {
        setBusy(true);
        setError(null);
        try {
            await azureApi.signIn();
            onSignedIn();
        } catch (failure) {
            setError(errorText(failure));
        } finally {
            setBusy(false);
        }
    };

    return (
        <div className="az-pane az-pane-empty">
            <EmptyState
                icon={<AzureMark size={28} className="icon-azure" />}
                title={missing ? "The Azure CLI isn't installed" : "Sign in to Azure"}
                message={
                    missing
                        ? "Sikemux signs in through the Azure CLI, so every tenant and subscription it knows works here. Install it, then sign in."
                        : (status?.message ??
                          "Sikemux signs in through the Azure CLI. This opens your browser; the CLI keeps the sign-in, not Sikemux.")
                }
                action={
                    missing
                        ? { label: "Copy the install command", onClick: () => copyWithNotice("brew install azure-cli", "The install command") }
                        : { label: busy ? "Waiting for the browser…" : "Sign in with the browser", onClick: () => void signIn() }
                }
            />
            <SignInError error={error} />
            {missing ? (
                <button type="button" className="az-link" onClick={() => void openUrl(INSTALL).catch(swallow("open Microsoft Learn"))}>
                    How to install it
                </button>
            ) : null}
        </div>
    );
}
