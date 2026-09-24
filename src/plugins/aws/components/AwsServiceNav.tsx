import { AWS_SERVICES, awsSettings, setAwsService, type AwsService } from "../state";

const META: Record<AwsService, { label: string; hint: string }> = {
    ecs: { label: "ECS", hint: "clusters · services · tasks · logs" },
    ec2: { label: "EC2", hint: "instances" },
    lambda: { label: "Lambda", hint: "functions" },
    sqs: { label: "SQS", hint: "queues" },
    billing: { label: "Billing", hint: "month-by-month" },
    s3: { label: "S3", hint: "buckets" },
};

export function AwsServiceNav() {
    const active = awsSettings.useSelect((s) => s.service);
    return (
        <nav className="aws-nav">
            <div className="aws-nav-label">Services</div>
            {AWS_SERVICES.map((s, i) => {
                const m = META[s];
                const sel = active === s;
                return (
                    <button key={s} className={`aws-nav-item${sel ? " active" : ""}`} onClick={() => setAwsService(s)} title={m.hint}>
                        <span className="aws-nav-key">{i + 1}</span>
                        <span className="aws-nav-name">{m.label}</span>
                    </button>
                );
            })}
        </nav>
    );
}
