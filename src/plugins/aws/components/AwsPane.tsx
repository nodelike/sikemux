import type { ComponentType } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { deriveAuthState, needsAuth } from "../auth";
import { awsIdentityR } from "../resources";
import { awsSettings, type AwsService } from "../state";
import { AwsServiceNav } from "./AwsServiceNav";
import { AwsEcsView } from "./AwsEcsView";
import { AwsBillingView, AwsEc2View, AwsLambdaView, AwsS3View, AwsSqsView } from "./AwsListViews";
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

export function AwsPane({ active }: { active: boolean }) {
    const profile = awsSettings.useSelect((s) => s.profile);
    const service = awsSettings.useSelect((s) => s.service);
    const identity = useResourceEnabled(active && !!profile, awsIdentityR, profile ?? "", false);
    const auth = deriveAuthState(profile, identity);

    if (auth.kind === "no-profile") return <AwsAuthEmpty mode="no-profile" />;
    if (needsAuth(auth)) {
        return <AwsAuthEmpty mode="unauthed" profile={auth.profile} />;
    }
    const p = auth.profile;
    const ServiceView = AWS_VIEW[service];
    return (
        <div className="aws-pane">
            <AwsServiceNav />
            <div className="aws-main">
                <ServiceView profile={p} active={active} />
            </div>
        </div>
    );
}
