// The shared view reads limits and current usage straight from the
// deployment (admin key + deployment URL), not from a cloud billing API, so
// it works unchanged here — same as in the single-deployment self-hosted
// dashboard. It supplies its own DeploymentSettingsLayout.
import { UsageLimitsView } from "@common/features/settings/components/UsageLimitsView";

export default function UsageLimits() {
  return <UsageLimitsView />;
}
