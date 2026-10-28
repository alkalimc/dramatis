// Preview-only: `?view=<name>` opens a given screen of the mock app, so a screenshot can
// reach it without clicking. Not compiled into a production build.
import type { Actions, Store } from "./store";

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

let applied = false;

export async function applyDevView(view: string, store: Store, actions: Actions) {
  // Strict mode runs the app's effects twice; a click or a toggle must happen once.
  if (applied) return;
  applied = true;
  const d = store.dispatch;
  await wait(600);
  const s = store.getState();
  const group = s.channels.find((c) => c.kind === "group");
  const direct = s.channels.find((c) => c.kind === "direct");
  switch (view) {
    case "light":
      actions.setTheme("light");
      return d({ type: "drawer", drawer: { kind: "system", tab: "root" } });
    case "menu":
      return d({ type: "menu", open: true });
    case "profile":
      return d({ type: "profile", mode: "view" });
    case "profile-edit":
      return d({ type: "profile", mode: "edit" });
    case "system":
      return d({ type: "drawer", drawer: { kind: "system", tab: "root" } });
    case "settings-quiet":
      return d({ type: "drawer", drawer: { kind: "system", tab: "quiet" } });
    case "endpoints":
      return d({ type: "drawer", drawer: { kind: "system", tab: "endpoints" } });
    case "contacts":
      return d({ type: "drawer", drawer: { kind: "contacts" } });
    case "newgroup":
      return d({ type: "groupDialog", open: true });
    case "find":
      return d({ type: "find", query: "" });
    case "attach":
      document.querySelector<HTMLElement>('button[aria-label="Attach"]')?.click();
      return;
    case "roster":
      return d({ type: "drawer", drawer: { kind: "roster" } });
    case "cases":
      return d({ type: "drawer", drawer: { kind: "cases" } });
    case "world":
      return d({ type: "drawer", drawer: { kind: "world" } });
    case "archived":
      if (group) await actions.archive(group.id, true);
      return d({ type: "listView", view: "archived" });
    case "group":
      if (!group) return;
      await actions.open(group.id);
      return d({ type: "drawer", drawer: { kind: "channel", id: group.id } });
    case "person":
      if (!direct) return;
      await actions.open(direct.id);
      return d({ type: "drawer", drawer: { kind: "person", id: direct.participants[0].id } });
    case "direct":
      if (direct) await actions.open(direct.id);
      return;
    case "ctx": {
      const row = document.querySelector<HTMLElement>("button.chat-row:not(.archived-row)");
      row?.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: 200, clientY: 140 }));
      return;
    }
    case "ctx-group": {
      await wait(400);
      const row = [...document.querySelectorAll<HTMLElement>("button.chat-row:not(.archived-row)")].find((r) => r.querySelector(".chat-name svg"));
      row?.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 200, clientY: 200 }));
      return;
    }
    case "ctx-msg": {
      if (direct) await actions.open(direct.id);
      await wait(400);
      const mine = document.querySelector<HTMLElement>(".message.from-user .bubble");
      mine?.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 1200, clientY: 140 }));
      return;
    }
    case "ctx-person": {
      if (direct) await actions.open(direct.id);
      await wait(400);
      const other = document.querySelectorAll<HTMLElement>(".message.from-other .bubble")[1];
      other?.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 500, clientY: 400 }));
      return;
    }
    case "sources": {
      const withCites = Object.entries(s.messages).find(([, list]) => list.some((m) => m.cites.length > 0));
      if (!withCites) return;
      await actions.open(withCites[0] === s.hostChannel ? null : withCites[0]);
      await wait(400);
      document.querySelector<HTMLElement>(".sources-toggle")?.click();
      return;
    }
  }
}
