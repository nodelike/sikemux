import { SignIns, type Load } from "./SignIns.tsx";
import { PageHead } from "./old.tsx";

export function ConnectorsPage({ signIns }: { signIns: Load }) {
  return (
    <>
      <PageHead title="Connectors">
        The services your agents signed in to from your account's vault in the
        last 30 days.
      </PageHead>
      <SignIns load={signIns} />
    </>
  );
}
