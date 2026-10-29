import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { Api } from "../api";
import { when } from "../lib/format";
import { renderApp } from "../test/render";

/** A corpus that words a few keys in another language (Spanish stands in for the corpus language); English mode drops them, as the engine does. */
function worded(api: Api): Api {
  const wording = async () => {
    const lang = await api.commands.interfaceLanguage();
    const entries: Record<string, string> =
      lang.status === "ok" && lang.data === "english"
        ? { "lang.corpus": "Espanol" }
        : { "settings.page.language": "Idioma", "menu.settings": "Ajustes", "lang.corpus": "Espanol", "first.title": "Bienvenida", "first.language": "Idioma", "first.bio": "Sobre ti", locale: "es-ES-u-hc-h23" };
    return { status: "ok" as const, data: { entries, scene_marker: { open: "*", close: "*" } } };
  };
  return { ...api, commands: { ...api.commands, wording } };
}

test("the language setting switches the words at once and is remembered", async () => {
  const user = userEvent.setup();
  const { api } = renderApp("", worded);
  await screen.findByRole("heading", { name: "Who's waiting" });
  await user.click(screen.getByRole("button", { name: "Menu" }));
  await user.click(within(await screen.findByRole("complementary", { name: "Menu" })).getByRole("button", { name: "Ajustes" }));
  const card = await screen.findByRole("dialog", { name: "Ajustes" });
  await user.click(within(card).getByRole("button", { name: /^Idioma/ }));
  const page = within(card).getByRole("region", { name: "Idioma" });
  expect(within(page).getByRole("radio", { name: "Espanol" })).toHaveProperty("checked", true);
  await user.click(within(page).getByRole("radio", { name: "English" }));
  // The card, and the rest of the screen, are in English with no reload.
  expect(await screen.findByRole("dialog", { name: "Settings" })).toBeDefined();
  expect(within(card).getByRole("radio", { name: "English" })).toHaveProperty("checked", true);
  const stored = await api.commands.interfaceLanguage();
  expect(stored.status === "ok" && stored.data).toBe("english");
  await user.click(within(card).getByRole("radio", { name: "Espanol" }));
  expect(await screen.findByRole("dialog", { name: "Ajustes" })).toBeDefined();
});

test("first open asks for the language and takes an optional bio", async () => {
  const user = userEvent.setup();
  const { api } = renderApp("first=1", worded);
  expect(await screen.findByRole("heading", { name: "Bienvenida" })).toBeDefined();
  await user.click(screen.getByRole("radio", { name: "English" }));
  expect(await screen.findByRole("heading", { name: "Welcome to the team" })).toBeDefined();
  await user.type(screen.getByLabelText("What should they call you?"), "Rowan");
  await user.type(screen.getByLabelText("About you (optional, you can skip this)"), "I draw maps.");
  await user.click(screen.getByRole("button", { name: "Continue" }));
  await waitFor(async () => {
    const s = await api.commands.settings();
    expect(s.status === "ok" && s.data.bio).toBe("I draw maps.");
  });
});

test("a bio over the limit cannot be submitted", async () => {
  const user = userEvent.setup();
  renderApp("first=1");
  await user.type(await screen.findByLabelText("What should they call you?"), "Rowan");
  const bio = screen.getByLabelText("About you (optional, you can skip this)");
  await user.click(bio);
  await user.paste("x".repeat(201));
  expect(screen.getByRole("button", { name: "Continue" })).toHaveProperty("disabled", true);
});

test("times follow the wording's locale, not the browser's", () => {
  const t = (k: string) => (k === "locale" ? "es-ES-u-hc-h23" : k);
  const now = new Date("2026-03-05T09:00:00");
  expect(when("2026-03-05T01:05:00", t as never, now)).toBe("01:05");
  expect(when("2026-03-01T01:05:00", t as never, now)).toMatch(/1 mar/);
  const en = (k: string) => (k === "locale" ? "en" : k);
  expect(when("2026-03-01T01:05:00", en as never, now)).toBe("Mar 1");
});
