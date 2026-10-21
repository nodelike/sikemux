import { errorText } from "../../../plugin-api/format";
import { useState } from "react";
import { notify } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, SkeletonRows } from "../../../plugin-api/ui";
import { gcpApi, type PeekedMessage, type Subscription, type Target, type Topic } from "../api";
import { metricsR, pubsubR } from "../resources";
import { updateGcpView, useGcpView } from "../state";
import type { ProductProps } from "./GcpPane";
import { Filter } from "./RunPage";
import { ago, Button, consoleUrl, Facts, keyOf, Loaded, PageHeader, Sparkline, Table, Tabs, visit } from "./parts";
import { Problem } from "./Problem";

const LISTS = [
    { value: "topics", label: "Topics" },
    { value: "subscriptions", label: "Subscriptions" },
] as const;

/** `key=value` lines as attributes; lines without `=` are skipped. */
export function attributesOf(text: string): Record<string, string> {
    return Object.fromEntries(
        text
            .split("\n")
            .map((line) => line.trim())
            .filter((line) => line.includes("="))
            .map((line) => [line.slice(0, line.indexOf("=")).trim(), line.slice(line.indexOf("=") + 1).trim()] as const)
            .filter(([key]) => key),
    );
}

export function PubSubPage({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    const list = view.list === "subscriptions" ? "subscriptions" : "topics";
    const pubsub = useResourceEnabled(active, pubsubR, ...keyOf(target));
    const words = view.filter.trim().toLowerCase();
    const open = (name: string) => updateGcpView(paneId, { open: view.open === name ? null : name });
    return (
        <div className="gcp-page">
            <PageHeader title="Pub/Sub">
                <Button onClick={() => visit(consoleUrl("cloudpubsub/topic/list", target.project))}>Create topic in Cloud console</Button>
            </PageHeader>
            <Tabs label="Pub/Sub" tabs={LISTS} value={list} onChange={(value) => updateGcpView(paneId, { list: value, open: null })} />
            <Filter paneId={paneId} placeholder={list === "topics" ? "Filter topics" : "Filter subscriptions"} />
            <Loaded
                handle={pubsub}
                label="topics"
                empty={(data) => (data.topics.length + data.subscriptions.length === 0 ? "No topics or subscriptions in this project." : null)}>
                {(data) => {
                    const topic = list === "topics" ? data.topics.find((each) => each.name === view.open) : undefined;
                    const subscription = list === "subscriptions" ? data.subscriptions.find((each) => each.name === view.open) : undefined;
                    return (
                        <div className={topic || subscription ? "gcp-split" : undefined}>
                            {list === "topics" ? (
                                <Table
                                    label="Topics"
                                    rows={data.topics.filter((each) => !words || each.name.toLowerCase().includes(words))}
                                    rowKey={(each) => each.name}
                                    selected={view.open}
                                    onOpen={(each: Topic) => open(each.name)}
                                    columns={[
                                        {
                                            label: "Topic ID",
                                            width: "minmax(160px, 1.4fr)",
                                            cell: (each) => <span className="gcp-name">{each.name}</span>,
                                        },
                                        {
                                            label: "Subscriptions",
                                            cell: (each) => data.subscriptions.filter((sub) => sub.topic === each.name).length,
                                        },
                                        { label: "Retention", cell: (each) => each.retention ?? "—" },
                                    ]}
                                />
                            ) : (
                                <Table
                                    label="Subscriptions"
                                    rows={data.subscriptions.filter(
                                        (each) => !words || each.name.toLowerCase().includes(words) || each.topic.includes(words),
                                    )}
                                    rowKey={(each) => each.name}
                                    selected={view.open}
                                    onOpen={(each: Subscription) => open(each.name)}
                                    columns={[
                                        {
                                            label: "Subscription ID",
                                            width: "minmax(160px, 1.4fr)",
                                            cell: (each) => <span className="gcp-name">{each.name}</span>,
                                        },
                                        { label: "Topic", cell: (each) => each.topic },
                                        { label: "Delivery", cell: (each) => each.kind },
                                        { label: "Ack deadline", cell: (each) => (each.ackDeadlineSeconds ? `${each.ackDeadlineSeconds}s` : "—") },
                                    ]}
                                />
                            )}
                            {topic && (
                                <TopicPanel
                                    target={target}
                                    topic={topic}
                                    subscriptions={data.subscriptions.filter((sub) => sub.topic === topic.name)}
                                    onClose={() => open(topic.name)}
                                />
                            )}
                            {subscription && (
                                <SubscriptionPanel
                                    active={active}
                                    target={target}
                                    subscription={subscription}
                                    onClose={() => open(subscription.name)}
                                />
                            )}
                        </div>
                    );
                }}
            </Loaded>
        </div>
    );
}

function TopicPanel({ target, topic, subscriptions, onClose }: { target: Target; topic: Topic; subscriptions: Subscription[]; onClose: () => void }) {
    const [data, setData] = useState("");
    const [attributes, setAttributes] = useState("");
    const [sending, setSending] = useState(false);
    const publish = () => {
        setSending(true);
        gcpApi
            .publish(target, topic.name, data, attributesOf(attributes))
            .then((sent) => {
                notify("success", `Published message ${sent.messageId}`);
                setData("");
            })
            .catch((failure: unknown) => notify("error", `Publish: ${errorText(failure)}`))
            .finally(() => setSending(false));
    };
    return (
        <aside className="gcp-panel" aria-label={`Topic ${topic.name}`}>
            <header>
                <h2>{topic.name}</h2>
                <button type="button" className="gcp-icon-button" aria-label="Close" onClick={onClose}>
                    ✕
                </button>
            </header>
            <Facts
                items={[
                    ["Subscriptions", subscriptions.map((sub) => sub.name).join(", ") || "None"],
                    ["Retention", topic.retention ?? "Default"],
                ]}
            />
            <h3 className="gcp-section-title">Publish message</h3>
            <textarea
                className="gcp-input gcp-filter"
                value={data}
                onChange={(event) => setData(event.target.value)}
                placeholder="Message body"
                aria-label="Message body"
            />
            <textarea
                className="gcp-input gcp-filter"
                value={attributes}
                onChange={(event) => setAttributes(event.target.value)}
                placeholder={"Attributes, one key=value a line"}
                aria-label="Attributes"
            />
            <div>
                <Button primary disabled={sending || !data} onClick={publish}>
                    {sending ? "Publishing…" : "Publish"}
                </Button>
            </div>
        </aside>
    );
}

function SubscriptionPanel({
    active,
    target,
    subscription,
    onClose,
}: {
    active: boolean;
    target: Target;
    subscription: Subscription;
    onClose: () => void;
}) {
    const key = keyOf(target);
    const backlog = useResourceEnabled(active, metricsR, ...key, "subscriptionBacklog", subscription.name, "6h");
    const oldest = useResourceEnabled(active, metricsR, ...key, "subscriptionOldest", subscription.name, "6h");
    const [messages, setMessages] = useState<PeekedMessage[] | null>(null);
    const [pulling, setPulling] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const pull = () => {
        setPulling(true);
        setError(null);
        gcpApi
            .peek(target, subscription.name)
            .then(setMessages)
            .catch((failure: unknown) => setError(errorText(failure)))
            .finally(() => setPulling(false));
    };
    return (
        <aside className="gcp-panel" aria-label={`Subscription ${subscription.name}`}>
            <header>
                <h2>{subscription.name}</h2>
                <button type="button" className="gcp-icon-button" aria-label="Close" onClick={onClose}>
                    ✕
                </button>
            </header>
            <Facts
                items={[
                    ["Topic", subscription.topic],
                    ["Delivery", subscription.kind === "push" ? `Push to ${subscription.pushEndpoint}` : subscription.kind],
                    [
                        "Dead-letter topic",
                        subscription.deadLetterTopic && `${subscription.deadLetterTopic} after ${subscription.maxDeliveryAttempts} attempts`,
                    ],
                    ["Filter", subscription.filter],
                    ["Ordering", subscription.ordering ? "On" : null],
                ]}
            />
            <div className="gcp-charts">
                <Sparkline title="Unacked messages" series={backlog.data} />
                <Sparkline title="Oldest unacked" series={oldest.data} />
            </div>
            {subscription.kind === "pull" && (
                <>
                    <div className="gcp-log-bar">
                        <Button primary disabled={pulling} onClick={pull}>
                            {pulling ? "Pulling…" : "View messages"}
                        </Button>
                        <span className="gcp-muted">Messages are handed straight back, so readers still get them.</span>
                    </div>
                    {error ? (
                        <Problem error={error} what="messages" onRetry={pull} />
                    ) : pulling && !messages ? (
                        <SkeletonRows rows={3} label="Pulling messages" />
                    ) : messages && messages.length === 0 ? (
                        <EmptyState message="No messages are waiting." />
                    ) : (
                        messages?.map((message) => (
                            <article key={message.id} className="gcp-message">
                                <header className="gcp-muted">
                                    {message.id} · {ago(message.publishedAt)}
                                    {message.deliveryAttempt ? ` · attempt ${message.deliveryAttempt}` : ""}
                                </header>
                                <pre>{"text" in message.data ? message.data.text : `base64: ${message.data.base64}`}</pre>
                                {message.attributes && Object.keys(message.attributes).length > 0 && (
                                    <span className="gcp-muted">
                                        {Object.entries(message.attributes)
                                            .map(([name, value]) => `${name}=${value}`)
                                            .join("  ")}
                                    </span>
                                )}
                            </article>
                        ))
                    )}
                </>
            )}
        </aside>
    );
}
