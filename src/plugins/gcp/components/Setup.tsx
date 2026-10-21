import { refreshGcp } from "../resources";
import { useState } from "react";
import { runInTerminal, swallow } from "../../../plugin-api/host";
import { SignInError, SignInScreen, SignInWaiting } from "../../../plugin-api/ui";
import { gcpApi, type GcpStatus } from "../api";
import { invalidate } from "../../../plugin-api/resources";
import { GcpMark } from "./icons";

const INSTALL = ["brew", "install", "--cask", "gcloud-cli"];

/** Before the console: getting the gcloud CLI, then signing in to it. */
export function Setup({ status }: { status: GcpStatus | undefined }) {
    const [waiting, setWaiting] = useState<null | (() => void)>(null);
    const [error, setError] = useState<string | null>(null);
    const signedIn = () => {
        setWaiting(null);
        invalidate((kind) => kind === "gcp.status");
        refreshGcp();
    };
    const signIn = () => {
        setError(null);
        const stream = gcpApi.signIn({
            onDone: signedIn,
            onError: (message) => {
                setWaiting(null);
                setError(message);
            },
        });
        setWaiting(() => () => stream.stop());
    };

    if (status && !status.installed) {
        return (
            <SignInScreen
                mark={<GcpMark size={28} />}
                title="Install the Google Cloud CLI"
                lede="Sikemux uses gcloud for your Google sign-in, so every account and project it knows works here, and no Google credential is stored by Sikemux."
                foot="After installing, open a new terminal so gcloud is on your PATH, then come back here.">
                <button
                    type="button"
                    className="signin-btn primary"
                    onClick={() =>
                        void runInTerminal("Install the Google Cloud CLI", INSTALL)
                            .then(() => invalidate((kind) => kind === "gcp.status"))
                            .catch(swallow("install gcloud"))
                    }>
                    Install with Homebrew
                </button>
                <p className="signin-hint">
                    Runs <code>{INSTALL.join(" ")}</code> in a terminal. Without Homebrew, follow cloud.google.com/sdk/docs/install.
                </p>
            </SignInScreen>
        );
    }

    return (
        <SignInScreen
            mark={<GcpMark size={28} />}
            title="Connect Google Cloud"
            lede="See your Cloud Run services, VMs, clusters, buckets, databases, builds and logs beside your code, and let agents read and act on them."
            foot="gcloud keeps your sign-in. Sikemux asks it for short-lived access tokens and never stores one.">
            {waiting ? (
                <SignInWaiting
                    onCancel={() => {
                        waiting();
                        setWaiting(null);
                    }}>
                    Finish signing in in your browser
                </SignInWaiting>
            ) : (
                <button type="button" className="signin-btn primary" onClick={signIn}>
                    <GcpMark size={14} />
                    Sign in with Google
                </button>
            )}
            <SignInError error={error} />
            <p className="signin-hint">
                Runs <code>gcloud auth login</code>. A service account works too: run{" "}
                <code>gcloud auth activate-service-account --key-file=KEY.json</code> in a terminal.
            </p>
        </SignInScreen>
    );
}
