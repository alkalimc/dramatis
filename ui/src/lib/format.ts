import type { Activity, Audience, Author, ToolAction } from "../api";
import type { T } from "../i18n";

export type Names = Record<string, string>;

export function authorName(a: Author, t: T, names: Names, userName: string | null): string {
  switch (a.kind) {
    case "user":
      return userName ? t("user.title", { name: userName }) : t("user.you");
    case "host":
      return t("host.name");
    case "person":
      return names[a.id] ?? t("person.unknown");
  }
}

/** A short, locale-formatted "when" for timestamps: time today, else date. */
export function when(iso: string, now: Date = new Date()): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const sameDay = d.toDateString() === now.toDateString();
  return sameDay
    ? d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })
    : d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
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

export function actionText(a: ToolAction, t: T, names: Names): string {
  const list = (ps: { name: string }[]) => ps.map((p) => p.name).join(", ");
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
      return t("action.create_group", { names: list(a.members) });
    case "set_mode":
      return t("action.set_mode", {
        target: a.target.kind === "person" ? (names[a.target.id] ?? t("person.unknown")) : t("action.target.group"),
        mode: t(`mode.${a.mode}`),
      });
    case "forget":
      return t("action.forget");
  }
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
