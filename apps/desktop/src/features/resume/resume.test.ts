// Every event kind has a localized resume label (audit r1: Phase 2/3 kinds
// showed raw English names such as "pipeline updated" in the Russian card).
import { EVENT_KINDS } from "@orkestr/protocol";
import { afterEach, describe, expect, it } from "vitest";

import { applyLanguagePreference, i18n } from "../../i18n";
import { describeCount, headline } from "./resume";

const nb = (s: string) => s.replace(/ /g, " ");

afterEach(async () => {
  await applyLanguagePreference("en");
});

describe("resume kinds", () => {
  it("has a plural set for every protocol event kind in en and ru", () => {
    const missing: string[] = [];
    for (const kind of EVENT_KINDS) {
      for (const key of [`en:${kind}_one`, `en:${kind}_other`]) {
        const [lng, k] = key.split(":") as [string, string];
        if (!i18n.exists(`resume:kinds.${k}`, { lng })) missing.push(key);
      }
      for (const form of ["one", "few", "many", "other"]) {
        if (!i18n.exists(`resume:kinds.${kind}_${form}`, { lng: "ru" })) missing.push(`ru:${kind}_${form}`);
      }
    }
    expect(missing).toEqual([]);
  });

  it("pipeline and review kinds read in Russian", async () => {
    await applyLanguagePreference("ru");
    expect(nb(describeCount({ kind: "PipelineUpdated", count: 60 }))).toBe("60 обновлений конвейера качества");
    expect(nb(describeCount({ kind: "PipelineStageChanged", count: 48 }))).toBe("48 смен этапов конвейера");
    expect(nb(describeCount({ kind: "ReviewUpdated", count: 32 }))).toBe("32 обновления ревью");
    expect(nb(describeCount({ kind: "ConsensusUpdated", count: 1 }))).toBe("1 обновление консенсуса");
  });
});

describe("resume headline", () => {
  const base = { since: "2026-10-01T00:00:00Z", previousSessionStartedAt: null, counts: [], totalEvents: 0, highlights: [] };

  it("a crash recovery is never reported as «nothing happened»", async () => {
    await applyLanguagePreference("ru");
    const text = headline({ ...base, recoveredFromCrash: true }, "shop");
    expect(text).toContain("восстановлен");
    expect(text).not.toContain("ничего не произошло");
    expect(headline({ ...base, recoveredFromCrash: false }, "shop")).toContain("ничего не произошло");
  });
});
