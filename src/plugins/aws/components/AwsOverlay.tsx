import { usePluginOverlay } from "../../../plugin-api/host";
import { useAws } from "../state";
import { AwsAuthModal } from "./AwsAuthModal";

export function AwsOverlay() {
    const open = useAws((state) => state.authModal !== null);
    usePluginOverlay(open);
    return open ? <AwsAuthModal /> : null;
}
