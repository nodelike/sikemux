import type { ComponentType, KeyboardEvent } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { deriveAuthState, needsAuth } from "../auth";
import { awsIdentityR } from "../resources";
import { AWS_SERVICES, awsSettings, setAwsService, setEcsLevel, setLambdaLogs, useAws, type AwsService } from "../state";
import { AwsServiceNav } from "./AwsServiceNav";
import { AwsEcsView } from "./AwsEcsView";
import { AwsBillingView } from "./AwsBillingView";
import { AwsEc2View, AwsLambdaView, AwsS3View, AwsSqsView } from "./AwsListViews";
import { AwsAuthEmpty } from "./AwsAuthEmpty";
import "../aws.css";

type AwsViewProps = { profile: string; active: boolean };

const AWS_VIEW: Record<AwsService, ComponentType<AwsViewProps>> = {
    ecs: AwsEcsView,
    ec2: AwsEc2View,
    lambda: AwsLambdaView,
    sqs: AwsSqsView,
    billing: AwsBillingView,
    s3: AwsS3View,
};

function typing(target: EventTarget): boolean {
    return target instanceof HTMLElement && (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName));
}

/** Esc climbs back out of whatever the current service has drilled into. */
function goBack(profile: string, service: AwsService): boolean {
    const state = useAws.getState();
    if (service === "lambda" && state.lambdaLogs[profile]) {
        setLambdaLogs(profile, null);
        return true;
    }
    const level = state.ecsViews[profile];
    if (service !== "ecs" || !level || level.kind === "clusters") return false;
    setEcsLevel(profile, level.kind === "service" ? { kind: "services", cluster: level.cluster } : { kind: "clusters" });
    return true;
}

export function AwsPane({ active }: { active: boolean }) {
    const profile = awsSettings.useSelect((s) => s.profile);
    const service = awsSettings.useSelect((s) => s.service);
    const identity = useResourceEnabled(active && !!profile, awsIdentityR, profile ?? "", false);
    const auth = deriveAuthState(profile, identity);

    if (auth.kind === "no-profile") return <AwsAuthEmpty mode="no-profile" />;
    const signedOut = needsAuth(auth);
    const ServiceView = AWS_VIEW[service];

    const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
        if (event.metaKey || event.ctrlKey || event.altKey) return;
        if (event.key === "Escape" && !typing(event.target) && goBack(auth.profile, service)) {
            event.preventDefault();
            return;
        }
        if (typing(event.target) || signedOut) return;
        const index = "123456".indexOf(event.key);
        if (index >= 0 && event.key.length === 1) {
            event.preventDefault();
            setAwsService(AWS_SERVICES[index]);
        } else if (event.key === "/") {
            const filter = event.currentTarget.querySelector<HTMLInputElement>(".aws-filter input");
            if (filter) {
                event.preventDefault();
                filter.focus();
                filter.select();
            }
        }
    };

    return (
        <div className="aws-pane" onKeyDown={onKeyDown}>
            <AwsServiceNav profile={auth.profile} signedIn={!signedOut} />
            {signedOut ? <AwsAuthEmpty mode="unauthed" profile={auth.profile} /> : <ServiceView profile={auth.profile} active={active} />}
        </div>
    );
}
