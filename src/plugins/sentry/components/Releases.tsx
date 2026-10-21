import type { Release } from "../api";
import { ago, count } from "../format";

export function healthTone(rate: number | null): "live" | "warn" | "danger" | "idle" {
    if (rate === null) return "idle";
    if (rate >= 99.5) return "live";
    if (rate >= 98) return "warn";
    return "danger";
}

function Rate({ rate }: { rate: number | null }) {
    if (rate === null) return <span className="sentry-muted">—</span>;
    return (
        <span className={`sentry-rate ${healthTone(rate)}`}>
            <span className="sentry-rate-value">{rate.toFixed(rate >= 99.95 ? 0 : 2)}%</span>
            <span className="sentry-rate-bar" aria-hidden="true">
                <span style={{ width: `${Math.max(2, rate)}%` }} />
            </span>
        </span>
    );
}

export function Releases({ releases, onIssues }: { releases: Release[]; onIssues: (release: Release) => void }) {
    if (releases.length === 0) return <p className="sentry-note">No releases yet. Sentry lists them once the SDK sends a release with its events.</p>;
    const health = releases.some((release) => release.crashFreeSessions !== null);
    return (
        <section className="sentry-section" aria-label="Releases">
            <table className="sentry-table" aria-label="Releases">
                <thead>
                    <tr>
                        <th>Release</th>
                        <th>Created</th>
                        <th className="sentry-num">New issues</th>
                        {health ? <th>Crash-free sessions</th> : null}
                        {health ? <th>Crash-free users</th> : null}
                        {health ? <th className="sentry-num">Sessions</th> : null}
                        <th>Last deploy</th>
                    </tr>
                </thead>
                <tbody>
                    {releases.map((release) => (
                        <tr key={release.version}>
                            <td className="sentry-mono sentry-release" title={release.version}>
                                {release.short}
                                {release.commits > 0 ? (
                                    <span className="sentry-release-commits">
                                        {release.commits} {release.commits === 1 ? "commit" : "commits"}
                                    </span>
                                ) : null}
                            </td>
                            <td className="sentry-muted">{ago(release.created)} ago</td>
                            <td className="sentry-num">
                                {release.newIssues > 0 ? (
                                    <button type="button" className="sentry-link" onClick={() => onIssues(release)}>
                                        {release.newIssues}
                                    </button>
                                ) : (
                                    <span className="sentry-muted">0</span>
                                )}
                            </td>
                            {health ? (
                                <td>
                                    <Rate rate={release.crashFreeSessions} />
                                </td>
                            ) : null}
                            {health ? (
                                <td>
                                    <Rate rate={release.crashFreeUsers} />
                                </td>
                            ) : null}
                            {health ? <td className="sentry-num">{release.sessions > 0 ? count(release.sessions) : "—"}</td> : null}
                            <td className="sentry-muted">
                                {release.deployedTo ? `${release.deployedTo}${release.deployedAt ? `, ${ago(release.deployedAt)} ago` : ""}` : "—"}
                            </td>
                        </tr>
                    ))}
                </tbody>
            </table>
        </section>
    );
}
