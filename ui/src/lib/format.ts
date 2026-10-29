import type { Activity, Audience, Author, ToolAction } from "../api";
import { userTitle, type T } from "../i18n";

export type Names = Record<string, string>;

export function authorName(a: Author, t: T, names: Names, userName: string | null): string {
  switch (a.kind) {
    case "user":
      return userTitle(t, userName);
    case "host":
      return t("host.name");
    case "person":
      return names[a.id] ?? t("person.unknown");
  }
}

/** A short "when" for timestamps, in the wording's locale (not the browser's): time today, else date. */
export function when(iso: string, t: T, now: Date = new Date()): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const locale = t("locale");
  const sameDay = d.toDateString() === now.toDateString();
  return sameDay
    ? d.toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit" })
    : d.toLocaleDateString(locale, { month: "short", day: "numeric" });
}

export function activityText(a: Activity, t: T): string {
  switch (a.kind) {
    case "messaged":
      return t("activity.messaged");
    case "replied":
      return t("activity.replied", { question: a.question });
    case "commitment_due":
      return t("activity.commitment_due", { text: a.text });
    case "came_by":
      return t("activity.came_by");
    case "birthday":
      return t("activity.birthday");
  }
}

/** Most names a list spells out before it counts the rest. */
const LIST_NAMES = 3;

export function nameList(names: string[], t: T): string {
  const shown = names.slice(0, LIST_NAMES).join(t("list.separator"));
  const more = names.length - LIST_NAMES;
  return more > 0 ? t("list.more", { names: shown, count: more }) : shown;
}

export function actionText(a: ToolAction, t: T, names: Names): string {
  const list = (ps: { name: string }[]) => nameList(ps.map((p) => p.name), t);
  switch (a.tool) {
    case "search":
      return t("action.search", { query: a.query });
    case "get_chunk":
      return t("action.get_chunk");
    case "request_join":
      return a.person
        ? t("action.request_join.person", { name: a.person.name })
        : t("action.request_join.topic", { topic: a.topic ?? "" });
    case "remember":
      return t("action.remember", { text: a.text });
    case "report":
      return t("action.report");
    case "wrapup":
      // Wrap-up is bookkeeping, not an action the user asked for; callers skip it.
      return "";
    case "digest":
      return t("action.digest");
    case "find_people":
      return t("action.find_people", { topic: a.topic });
    case "ask":
      return t("action.ask", { name: a.person.name, question: a.question });
    case "create_group":
      // No channel: a suggestion that waits for the user, nothing made yet.
      return t(a.channel ? "action.create_group" : "action.suggest_group", { names: list(a.members) });
    case "set_mode":
      return t("action.set_mode", {
        target: a.target.kind === "person" ? (names[a.target.id] ?? t("person.unknown")) : t("action.target.group"),
        mode: t(`mode.${a.mode}`),
      });
    case "forget":
      return t("action.forget");
    case "list_channels":
      return t("action.list_channels");
  }
}

/**
 * What the user can take from a message: go to people found, groups made or listed, or make
 * a group someone suggested (`new_group`; nothing exists until the user takes it).
 */
export type Choice =
  | { kind: "person"; id: string; name: string }
  | { kind: "group"; channel: string; names: string[] }
  | { kind: "new_group"; members: string[]; names: string[]; action: number };

export function choicesOf(actions: ToolAction[]): Choice[] {
  const out: Choice[] = [];
  const seen = new Set<string>();
  actions.forEach((a, index) => {
    const found: Choice[] =
      a.tool === "find_people"
        ? (a.persons ?? []).map((p) => ({ kind: "person", id: p.id, name: p.name }))
        : a.tool === "create_group" && a.channel
          ? [{ kind: "group", channel: a.channel, names: a.members.map((m) => m.name) }]
          : a.tool === "create_group" && a.members.length >= 2
            ? [{ kind: "new_group", members: a.members.map((m) => m.id), names: a.members.map((m) => m.name), action: index }]
            : a.tool === "list_channels"
              ? (a.channels ?? []).flatMap((c): Choice[] =>
                  c.kind === "group"
                    ? [{ kind: "group", channel: c.id, names: c.members.map((m) => m.name) }]
                    : c.kind === "direct" && c.members[0]
                      ? [{ kind: "person", id: c.members[0].id, name: c.members[0].name }]
                      : [],
                )
              : [];
    for (const c of found) {
      const key = c.kind === "person" ? `p:${c.id}` : c.kind === "group" ? `g:${c.channel}` : `n:${c.members.join(",")}`;
      if (!seen.has(key)) {
        seen.add(key);
        out.push(c);
      }
    }
  });
  return out;
}

export function audienceText(a: Audience, t: T): string {
  switch (a.kind) {
    case "world":
      return t("world.audience.world");
    case "participants":
      return t("world.audience.participants", { names: a.persons.map((p) => p.name).join(", ") || t("host.name") });
    case "own":
      return t("world.audience.own", { name: a.person.name });
  }
}

/** "<name> is typing…" for everyone generating in a channel, or `null` when nobody is. */
export function typingText(who: Record<string, { author: Author; tool: boolean }> | undefined, t: T, names: Names, userName: string | null): string | null {
  const lines = Object.values(who ?? {}).map((w) => t(w.tool ? "typing.tool" : "typing.writing", { name: authorName(w.author, t, names, userName) }));
  return lines.length ? lines.join(t("list.separator")) : null;
}

/** Whether two authors are the same speaker (one run of bubbles). */
export function sameAuthor(a: Author, b: Author): boolean {
  return a.kind === b.kind && (a.kind !== "person" || (b.kind === "person" && a.id === b.id));
}
