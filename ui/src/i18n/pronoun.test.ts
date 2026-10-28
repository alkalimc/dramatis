import { expect, test } from "vitest";
import { makeT, pronounOf } from "./index";

test("a sentence about a person takes the pack's pronoun for their gender, else the neutral one", () => {
  const t = makeT({ entries: { "pronoun.female": "she", "pronoun.male": "he", "person.ask": "Ask {pronoun}" } });
  const ask = (pronoun?: "female" | "male" | null) => t("person.ask", pronounOf(t, { pronoun }));
  expect(ask("female")).toBe("Ask she");
  expect(ask("male")).toBe("Ask he");
  expect(ask(null)).toBe("Ask them");
  expect(ask()).toBe("Ask them");
});
