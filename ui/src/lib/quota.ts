import type { QuotaStatus } from "../api";
import type { T } from "../i18n";
import { when } from "./format";

/** The host's fixed line for a used-up quota, or null while calls still go through. */
export function quotaExhaustedLine(quota: QuotaStatus | null, t: T): string | null {
  if (!quota || quota.band !== "exhausted") return null;
  const at = quota.window?.release_at;
  return t("host.quota_exhausted", { when: at ? when(at, t) : t("quota.release.unknown") });
}
