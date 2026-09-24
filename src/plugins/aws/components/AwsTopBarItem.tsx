import { useResourceEnabled } from "../../../plugin-api/resources";
import { IconAws, Tooltip } from "../../../plugin-api/ui";
import { awsIdentityR } from "../resources";
import { awsSettings, openAwsAuthModal, openAwsSession } from "../state";

export function AwsTopBarItem() {
    const profile = awsSettings.useSelect((settings) => settings.profile);
    const identity = useResourceEnabled(!!profile, awsIdentityR, profile ?? "", false);
    const status = profile ? identity.data?.status : undefined;

    const dotClass =
        status === "authed"
            ? "ok"
            : identity.status === "loading"
              ? "checking"
              : status === "expired" || status === "no-credentials"
                ? "fail"
                : !profile
                  ? "off"
                  : "warn";

    const onClick = () => {
        if (profile && (status === "expired" || status === "no-credentials")) openAwsAuthModal(profile, null);
        else openAwsSession();
    };

    const title = profile ? `AWS · ${profile}${status ? ` · ${status}` : ""}` : "AWS — sign in";

    return (
        <Tooltip label={title}>
            <button className={`tb-aws-chip ${dotClass}`} onClick={onClick} aria-label={title}>
                <IconAws />
            </button>
        </Tooltip>
    );
}
