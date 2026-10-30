// In-memory stand-in for the engine, behind the same signatures as the generated
// commands. Used outside the Tauri webview (browser preview, tests). Replies are canned
// and streamed through the same events the engine sends.
//
// Demo switches (query string): `form=library` (no endpoint yet), `first=1` (first open), `long=1` (a long host history),
// `tts=0` (no read-aloud role), `tier=ultra`, `band=quiet|exhausted`, `marker=paren`
// (scene lines in `(...)` instead of `*...*`). Sending a message that contains the word
// `fail` ends the turn in a rate-limit failure instead of a reply. First-run switches:
// `nosave=1` (no save open: the picker), `saves=0` (and none exist), `missing=NAME` (the
// last used save is gone), `corpus=missing` (no corpus yet; a path with `newer` in it is
// from a newer format, one not ending in `.folio` is not a corpus), `dialog=1` (a file
// dialog exists). Update check: `update=1` (a newer release was already found), `update=fail`
// (the check cannot reach GitHub); "Check now" finds a newer release otherwise.
//
// `ui/wording.local.json` (git-ignored; exported by the forge) supplies a corpus's wording
// when present, so the dev-server preview shows that corpus's language. Only the dev
// server reads it: a production build never carries a local pack's wording.
import type {
  ApiError,
  AppStatus,
  AppInfo,
  Author,
  InterfaceLanguage,
  ChannelId,
  ChannelSummary,
  Citation,
  Confidence,
  EndpointSetup,
  EndpointsView,
  FactFilter,
  FactId,
  FactList,
  FactView,
  MediaFile,
  Message,
  Mode,
  PersonId,
  PersonRef,
  Profile,
  QuotaBand,
  QuotaStatus,
  Roster,
  RosterBand,
  SaveInfo,
  StorageCategory,
  StorageKind,
  StorageUsage,
  RosterRow,
  Settings,
  Tier,
  ToolAction,
  UpdateStatus,
  Waiting,
} from "../../api.gen";
import { version } from "../../../package.json";
import type { Commands, Events } from "..";
import { createBus } from "./bus";
import { PASSAGES, PERSONS, passage, type MockPerson } from "./data";

type Result<T> = Promise<{ status: "ok"; data: T } | { status: "error"; error: ApiError }>;

// Tests expect the English defaults, so they never read the local file.
const localWording: Record<string, Record<string, string>> =
  import.meta.env.DEV && import.meta.env.MODE !== "test"
    ? import.meta.glob<Record<string, string>>("/wording.local.json", {
        eager: true,
        import: "default",
      })
    : {};

const ok = <T>(data: T): Result<T> => Promise.resolve({ status: "ok", data });
const fail = <T>(error: ApiError): Result<T> => Promise.resolve({ status: "error", error });

const iso = (minutesAgo = 0) => new Date(Date.now() - minutesAgo * 60_000).toISOString();
const later = (minutes: number) => iso(-minutes);

const PRESETS: Profile[] = [
  { name: "deepseek", base_url: "https://api.deepseek.com", wire_api: "chat", model: [] },
  { name: "openai-compatible", base_url: "https://api.openai.com/v1", wire_api: "chat", model: [] },
  { name: "anthropic-compatible", base_url: "https://api.anthropic.com/v1", wire_api: "messages", model: [] },
  { name: "local", base_url: "http://127.0.0.1:8080/v1", wire_api: "chat", model: [] },
];

/** A few milliseconds of silence, so "play" has something real to load. */
function silentWav(): string {
  const samples = 800;
  const bytes = new Uint8Array(44 + samples);
  const view = new DataView(bytes.buffer);
  const text = (at: number, s: string) => [...s].forEach((c, i) => view.setUint8(at + i, c.charCodeAt(0)));
  text(0, "RIFF");
  view.setUint32(4, 36 + samples, true);
  text(8, "WAVEfmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, 8000, true);
  view.setUint32(28, 8000, true);
  view.setUint16(32, 1, true);
  view.setUint16(34, 8, true);
  text(36, "data");
  view.setUint32(40, samples, true);
  bytes.fill(128, 44);
  let bin = "";
  bytes.forEach((b) => (bin += String.fromCharCode(b)));
  return `data:audio/wav;base64,${btoa(bin)}`;
}

export interface MockOptions {
  /** Milliseconds between streamed chunks; 0 in tests. */
  delay?: number;
}

export function createMock(params: URLSearchParams, opts: MockOptions = {}): { commands: Commands; events: Events } {
  const delay = opts.delay ?? 35;
  const bus = createBus();
  const wait = (ms: number) => new Promise((r) => setTimeout(r, delay === 0 ? 0 : ms));

  const scene = params.get("marker") === "paren" ? { open: "(", close: ")" } : { open: "*", close: "*" };
  const sc = (s: string) => `${scene.open}${s}${scene.close}`;

  // ---- world ----
  const persons = new Map(PERSONS.map((p) => [p.id, { ...p }]));
  const ref = (id: PersonId): PersonRef => ({ id, name: id === "host" ? "Host" : (persons.get(id)?.name ?? id) });

  let library = params.get("form") === "library";
  let language: InterfaceLanguage = "corpus";
  let tts = params.get("tts") !== "0";
  // The first-run questions were seen (answered or skipped) on this save.
  let introduced = false;
  let settings: Settings = {
    user_name: params.get("first") === "1" ? null : "Rowan",
    birthday: null,
    bio: null,
    quiet_hours: { from: "23:00", to: "07:00" },
    notifications: true,
    events_per_day: 1,
  };
  let away = false;
  let introPending = settings.user_name === null;
  let openSave: string | null = params.get("nosave") === "1" ? null : "default";
  let corpusLoaded = params.get("corpus") !== "missing";
  // `?locked=1` starts on an encrypted save; its password is "hunter2".
  const repository = "https://github.com/alkalimc/dramatis";
  const info: AppInfo = {
    version,
    author: "alkali",
    author_url: "https://github.com/alkalimc",
    repository,
    license: "Apache-2.0",
    corpus_credits: { by: "the demo team", url: "https://example.org/demo-team", project: "https://example.org/demo-corpus", licence: "CC BY-NC-SA 4.0" },
  };
  const found = (auto: boolean): UpdateStatus => ({ current: version, latest: "99.0.0", url: `${repository}/releases/tag/v99.0.0`, newer: true, checked_at: iso(), auto, asked: true });
  let update: UpdateStatus = params.get("update") === "1" ? found(true) : { current: version, latest: null, url: null, newer: false, checked_at: null, auto: false, asked: params.get("first") !== "1" && params.get("update") !== "ask" };
  let locked = params.get("locked") === "1";
  // `?changed=1` starts as if another window had replaced the open save's file.
  let changedElsewhere = params.get("changed") === "1";
  let saves: SaveInfo[] =
    params.get("saves") === "0"
      ? []
      : [
          { name: "default", size_kb: 412, played_at: iso(5), current: true, encrypted: locked },
          { name: "second try", size_kb: 96, played_at: iso(60 * 24 * 3), current: false, encrypted: false },
        ];
  /** The two caches the user may clear; the other kinds are fixed sizes. */
  const cache = { avatars: { kb: 2300, files: 24 }, audio: { kb: 5200, files: 9 } };
  const storageUsage = (): StorageUsage => {
    const pictures = { count: 3, kb: 180 };
    const rows: [StorageKind, number, number, boolean][] = [
      ["saves", saves.reduce((n, s) => n + s.size_kb, 0) + pictures.kb, saves.length, false],
      ["avatars", cache.avatars.kb, cache.avatars.files, true],
      ["audio", cache.audio.kb, cache.audio.files, true],
      ["uploads", 120, 2, false],
      ["corpus", 48000, 1, false],
    ];
    const categories: StorageCategory[] = rows.map(([kind, kb, files, clearable]) => ({ kind, kb, files, clearable }));
    return {
      total_kb: categories.reduce((n, c) => n + c.kb, 0),
      categories,
      saves: saves.map((s) => ({ name: s.name, kb: s.size_kb, current: s.name === openSave })),
      pictures: pictures.count,
      picture_kb: pictures.kb,
      cache_limit_kb: 3_000_000,
    };
  };
  /** Passwords of encrypted saves, and the ones entered in this session. */
  const passwords = new Map<string, string>(locked ? [["default", "hunter2"]] : []);
  const known = new Set<string>(locked ? [] : saves.map((s) => s.name));
  const saveNameError = (name: string): ApiError | null => {
    if (!name.trim() || name !== name.trim() || /[/\\:*?"<>|]/.test(name) || name.startsWith(".")) {
      return { code: "invalid", field: "name" };
    }
    return saves.some((s) => s.name.toLowerCase() === name.toLowerCase()) ? { code: "exists" } : null;
  };
  let greeted = false;
  let tier: Tier = params.get("tier") === "ultra" ? "ultra" : "middle";
  const band: QuotaBand =
    params.get("band") === "quiet" ? "quiet" : params.get("band") === "exhausted" ? "exhausted" : "open";

  let userAvatar: MediaFile | null = null;
  let seq = 0;
  const nextId = (prefix: string) => `${prefix}-${++seq}`;

  interface Chan extends ChannelSummary {
    log: Message[];
  }
  const channels = new Map<ChannelId, Chan>();
  const addChannel = (c: Omit<Chan, "log" | "unread" | "last_at" | "name" | "avatar" | "archived">) => {
    const ch: Chan = { ...c, unread: 0, name: null, avatar: null, archived: false, last_at: null, log: [] };
    channels.set(c.id, ch);
    return ch;
  };
  const msg = (channel: ChannelId, author: Author, text: string, minutesAgo: number, extra: Partial<Message> = {}): Message => {
    // A person speaking brings an archived channel back, as the engine does.
    const home = channels.get(channel);
    if (home && author.kind === "person") home.archived = false;
    const m: Message = {
      id: nextId("m"),
      channel,
      author,
      text,
      actions: [],
      attachment: null,
      picture_lost: false,
      cites: [],
      confidence: null,
      relayed: false,
      at: iso(minutesAgo),
      ...extra,
    };
    const ch = channels.get(channel)!;
    ch.log.push(m);
    ch.last_at = m.at;
    return m;
  };
  const cite = (chunk: string, revid?: number): Citation => {
    const c = passage(chunk)!.citation;
    return revid === undefined ? c : { ...c, revid };
  };
  const person = (id: PersonId): Author => ({ kind: "person", id });
  const USER: Author = { kind: "user" };
  const HOST: Author = { kind: "host" };

  const direct = (id: PersonId) => {
    const cid = `dm:${id}`;
    if (!channels.has(cid)) {
      addChannel({ id: cid, kind: "direct", origin: "user", participants: [ref(id)], topic: null, mode: persons.get(id)!.mode });
    }
    return cid;
  };

  addChannel({ id: "host", kind: "host", origin: "system", participants: [], topic: null, mode: "enabled" });
  const group = addChannel({
    id: "g-rota",
    kind: "group",
    origin: "user",
    participants: [ref("oren"), ref("mira"), ref("tess")],
    topic: "Ward rota",
    mode: "frozen",
  });

  // ---- facts ----
  const facts = new Map<FactId, FactView>();
  const addFact = (f: Omit<FactView, "id" | "created_at" | "delivered" | "retracted"> & Partial<FactView>, minutesAgo = 0) => {
    const fact: FactView = { id: nextId("f"), delivered: false, retracted: false, created_at: iso(minutesAgo), ...f };
    facts.set(fact.id, fact);
    return fact;
  };

  if (!introPending) {
    // `?long=1`: 120 older lines in the host channel, so history needs more than one page.
    if (params.get("long") === "1") {
      for (let i = 1; i <= 120; i++) msg("host", i % 2 ? USER : HOST, `Older line ${i}`, 60 * 48 + (121 - i));
    }
    msg("host", USER, "Who knows about the harbor flood?", 60 * 26);
    msg(
      "host",
      HOST,
      "Mira Vale keeps the flood ledgers; Oren was on the ward that night.",
      60 * 26 - 1,
      {
        actions: [
          { tool: "find_people", topic: "harbor flood", persons: [ref("mira"), ref("oren")] },
        ],
      },
    );

    msg(direct("mira"), USER, "Morning. Busy day at the archive?", 60 * 48);
    msg(direct("mira"), person("mira"), `${sc("Mira sets down a stack of ledgers.")}\nAlways. The tide books don't file themselves.`, 60 * 48 - 2);
    msg(
      direct("mira"),
      person("mira"),
      `${sc("She runs a finger down the index.")}\nThe east corridor was sealed in the **first month** after the gates failed. My own ledger says so.\nOren remembers that night differently: he says it was high summer.`,
      60 * 20,
      {
        actions: [
          { tool: "search", query: "east corridor sealed flood" },
          { tool: "request_join", person: ref("oren"), topic: null },
        ],
        cites: [cite("c-ledger-1"), cite("c-ward-1", 2201)],
      },
    );

    msg(direct("oren"), person("oren"), "You said you'd bring the new rota by today. No rush, just reminding you.", 60 * 9);
    msg(direct("oren"), USER, "Mira says the corridor closed in the first month. What do you remember?", 60 * 8);
    msg(
      direct("oren"),
      person("oren"),
      `${sc("Oren frowns.")}\nIf Mira wrote that down, she has the ledger. But I was on that ward, and it was hot. I remember it differently: the evacuation order came late, not the warning.`,
      60 * 8 - 1,
      { relayed: true },
    );

    msg(direct("tess"), USER, "Which route is safest after dark?", 60 * 50);
    msg(direct("tess"), person("tess"), "Take the stairs by the fish market, never the bridge after dark.", 60 * 49, {
      actions: [{ tool: "search", query: "dock routes after dark" }],
      cites: [cite("c-docks-1")],
    });

    msg(group.id, USER, "Did the rota change today?", 60 * 3);
    msg(group.id, person("oren"), "It did. I'm on nights this week.", 60 * 3 - 1);
    msg(group.id, person("tess"), sc("Tess drops a parcel on the table."), 60 * 3 - 2);
    msg(group.id, person("tess"), "I carried the new rota up this morning. Kael counted the copies twice.", 60 * 3 - 2);
    msg(group.id, USER, "Tess, who built the old bridge?", 60 * 2);
    msg(group.id, person("tess"), "The old bridge? I don't know much about how it was built. Juno might.", 60 * 2 - 1, {
      actions: [{ tool: "search", query: "old bridge builder" }, { tool: "invite_to_group", channel: group.id, persons: [ref("juno")] }],
      confidence: "low",
    });

    addFact({ author: person("oren"), audience: { kind: "participants", channel: group.id, persons: group.participants }, kind: "fact", about: null, due: null, text: "Oren mentioned the east ward changed its rota this week." }, 60 * 3);
    addFact({ author: USER, audience: { kind: "participants", channel: direct("oren"), persons: [ref("oren")] }, kind: "commitment", about: { kind: "person", id: "oren" }, due: iso(60), text: "You said you'd bring Oren the new rota today." }, 60 * 30);
    addFact({ author: USER, audience: { kind: "participants", channel: direct("mira"), persons: [ref("mira")] }, kind: "hurt", about: { kind: "person", id: "mira" }, due: null, text: "Your ledgers are useless, nobody reads them." }, 60 * 47);
    addFact({ author: USER, audience: { kind: "participants", channel: direct("mira"), persons: [ref("mira")] }, kind: "fact", about: { kind: "user" }, due: null, text: "You said you don't like sweets.", retracted: true }, 60 * 46);
    addFact({ author: person("mira"), audience: { kind: "own", person: ref("mira") }, kind: "fact", about: { kind: "user" }, due: null, text: "Mira noticed you prefer short answers." }, 60 * 45);
  }

  const digestItems: Waiting[] = introPending
    ? []
    : [
        { person: ref("mira"), channel: direct("mira"), activity: { kind: "messaged", at: iso(60 * 20) } },
        { person: ref("oren"), channel: direct("oren"), activity: { kind: "commitment_due", fact: [...facts.values()].find((f) => f.kind === "commitment")!.id, text: "Bring Oren the new rota", due: iso(60) } },
        { person: ref("tess"), channel: direct("tess"), activity: { kind: "came_by", at: iso(60 * 11) } },
        { person: ref("juno"), channel: direct("juno"), activity: { kind: "birthday" } },
      ];

  // ---- replies ----
  function announceWorld(...changes: Parameters<typeof bus.worldChanged.emit>[0]["changes"]) {
    bus.worldChanged.emit({ changes });
  }

  async function stream(channel: ChannelId, author: Author, text: string, extra: Partial<Message> = {}) {
    const id = nextId("m");
    bus.turnActivity.emit({ channel, author, state: { kind: "thinking" } });
    await wait(300);
    for (const action of extra.actions ?? []) {
      bus.turnActivity.emit({ channel, author, state: { kind: "tool", name: action.tool } });
      bus.toolCalled.emit({ channel, message: id, author, action });
      await wait(450);
    }
    bus.turnActivity.emit({ channel, author, state: { kind: "writing" } });
    for (let i = 0; i < text.length; i += 14) {
      bus.messageDelta.emit({ channel, message: id, author, delta: text.slice(i, i + 14) });
      await wait(40);
    }
    const m = msg(channel, author, text, 0, extra);
    // `msg` minted its own id; keep the streamed one stable.
    const ch = channels.get(channel)!;
    ch.log[ch.log.length - 1] = { ...m, id };
    bus.messageAdded.emit({ message: { ...m, id } });
    bus.turnActivity.emit({ channel, author, state: { kind: "done" } });
    return id;
  }

  /** A turn that starts and then fails, as an endpoint refusing the call would. */
  async function failTurn(ch: Chan) {
    const author: Author = ch.kind === "host" ? HOST : person(ch.participants[0].id);
    bus.turnActivity.emit({ channel: ch.id, author, state: { kind: "thinking" } });
    await wait(400);
    bus.turnActivity.emit({ channel: ch.id, author, state: { kind: "failed", reason: "rate_limited" } });
  }

  function knows(p: MockPerson, topic: string): string | null {
    for (const [re, reason] of p.knows) if (re.source !== ".*" && re.test(topic)) return reason;
    return null;
  }

  function candidates(topic: string, exclude: PersonId[] = []) {
    if (!topic.trim()) return [];
    const hits = [...persons.values()]
      .filter((p) => p.mode !== "disabled" && !exclude.includes(p.id))
      .map((p) => ({ p, reason: knows(p, topic) }))
      .filter((x): x is { p: MockPerson; reason: string } => x.reason !== null);
    const archivist = persons.get("mira")!;
    if (hits.length === 0 && archivist.mode !== "disabled" && !exclude.includes("mira")) {
      hits.push({ p: archivist, reason: archivist.knows[archivist.knows.length - 1][1] });
    }
    return hits.slice(0, 5).map(({ p, reason }) => ({
      person: ref(p.id),
      mode: p.mode,
      reason,
      citation: PASSAGES.find((x) => x.persons.some((q) => q.id === p.id))?.citation ?? null,
    }));
  }

  function mentioned(text: string, among: PersonId[]): PersonId | undefined {
    const low = text.toLowerCase();
    return among.find((id) => {
      const name = persons.get(id)!.name.toLowerCase();
      return low.includes(name) || low.includes(name.split(" ")[0]);
    });
  }

  function canned(p: MockPerson, text: string): { text: string; extra: Partial<Message> } {
    const hit = PASSAGES.find((x) => x.persons.some((q) => q.id === p.id));
    const first = p.name.split(" ")[0];
    if (hit && knows(p, text)) {
      return {
        text: `${sc(`${first} thinks for a moment.`)}\nFrom what I've seen: ${hit.text.split(". ")[0]}.`,
        extra: { cites: [hit.citation] },
      };
    }
    if (/\?$/.test(text.trim())) {
      return {
        text: "I'm not sure. That isn't something I've dealt with.",
        extra: { actions: [{ tool: "search", query: text.slice(0, 40) }], confidence: "low" },
      };
    }
    return { text: `${sc(`${first} nods.`)}\nNoted. Anything else on your mind?`, extra: {} };
  }

  async function replyHost(text: string) {
    const actions: ToolAction[] = [];
    let reply: string;
    const named = mentioned(text, [...persons.keys()]);
    if (/\benable\b/i.test(text) && named) {
      actions.push({ tool: "set_mode", target: { kind: "person", id: named }, mode: "enabled" });
      persons.get(named)!.mode = "enabled";
      reply = `Done. ${persons.get(named)!.name} may now come to you on their own when something is on their mind.`;
    } else if (/\bremember\b|\bnote\b/i.test(text)) {
      const note = text.replace(/.*?(remember|note)( that)?/i, "").trim() || text;
      actions.push({ tool: "remember", text: note });
      addFact({ author: HOST, audience: { kind: "participants", channel: "host", persons: [] }, kind: "fact", about: { kind: "user" }, due: null, text: note });
      reply = "Noted. You can find it in the world drawer.";
    } else if (/\bwho\b|\bknows?\b|\bask\b|\bfind\b/i.test(text)) {
      const found = candidates(text);
      actions.push({ tool: "find_people", topic: text.slice(0, 60), persons: found.map((c) => c.person) });
      reply = found.length
        ? `These could help:\n\n${found.map((c) => `- **${c.person.name}**: ${c.reason}`).join("\n")}\n\nWant me to ask one of them?`
        : "Nobody on the roster has material on that.";
    } else if (/\bnew\b|\bwhat's up\b/i.test(text)) {
      actions.push({ tool: "digest" });
      reply = digestItems.length ? "Mira answered your question and Oren is waiting on the rota." : "Nothing new.";
    } else {
      reply = "I'm here. Ask me to find someone, or to note something down.";
    }
    await stream("host", HOST, reply, { actions });
    if (actions.some((a) => a.tool === "set_mode")) {
      bus.modeChanged.emit({ target: { kind: "person", id: named! }, mode: "enabled" });
      bus.rosterChanged.emit({ persons: [named!] });
    }
    if (actions.some((a) => a.tool === "remember")) announceWorld({ kind: "fact", id: "" });
  }

  async function replyChannel(ch: Chan, text: string) {
    if (ch.kind === "direct") {
      const p = persons.get(ch.participants[0].id)!;
      const r = canned(p, text);
      await stream(ch.id, person(p.id), r.text, r.extra);
      p.lastSeenMin = 0;
      return;
    }
    // Group: the named person, else the best material match, else the last speaker.
    const ids = ch.participants.map((p) => p.id).filter((id) => persons.get(id)!.mode !== "disabled");
    const last = [...ch.log].reverse().find((m) => m.author.kind === "person");
    const addressed =
      mentioned(text, ids) ??
      ids.find((id) => knows(persons.get(id)!, text)) ??
      (last && last.author.kind === "person" ? last.author.id : ids[0]);
    if (!addressed) return;
    const r = canned(persons.get(addressed)!, text);
    await stream(ch.id, person(addressed), r.text, r.extra);
    if (ch.mode === "enabled" && band === "open") {
      const other = ids.find((id) => id !== addressed && persons.get(id)!.mode === "enabled" && knows(persons.get(id)!, text));
      if (other) await stream(ch.id, person(other), "If I may add: I've seen something about that too.");
    }
  }

  // What each intensity allows a day: visits and daily events (the engine's table).
  const PACE: Record<Tier, { visits_per_day: number; events_max: number }> = {
    low: { visits_per_day: 2, events_max: 2 },
    middle: { visits_per_day: 5, events_max: 4 },
    high: { visits_per_day: 6, events_max: 5 },
    extra_high: { visits_per_day: 7, events_max: 6 },
    max: { visits_per_day: 8, events_max: 6 },
    ultra: { visits_per_day: 10, events_max: 6 },
  };
  function quotaStatus(): QuotaStatus {
    if (tier === "ultra") return { tier, band: "open", window: null, spent: spend(), ...PACE[tier] };
    const used = band === "open" ? 0.42 : band === "quiet" ? 0.86 : 1.0;
    return {
      tier,
      band,
      window: { used, release_at: band === "open" ? null : later(band === "quiet" ? 20 : 45) },
      spent: spend(),
      ...PACE[tier],
    };
  }
  function spend() {
    return [
      { shape: "direct" as const, calls: 14 },
      { shape: "group" as const, calls: 6 },
      { shape: "host" as const, calls: 3 },
      { shape: "opening" as const, calls: 1 },
      { shape: "wrapup" as const, calls: 2 },
    ];
  }

  const gate = (): ApiError | null => {
    if (library) return { code: "no_endpoint" };
    if (band === "exhausted" && tier !== "ultra") return { code: "quota_exhausted", release_at: later(125) };
    return null;
  };

  let endpoints: EndpointsView = library
    ? { profiles: [], roles: null, presets: PRESETS }
    : {
        profiles: [
          { profile: { name: "primary", base_url: "https://llm.example.invalid/v1", wire_api: "responses", model: [{ id: "model-a" }] }, has_key: true },
          { profile: { name: "deepseek", base_url: "https://api.deepseek.com", wire_api: "chat", model: [] }, has_key: false },
        ],
        roles: { chat: { profile: "primary", model: "model-a" }, tts: tts ? { profile: "primary", model: "tts-1" } : null },
        presets: PRESETS,
      };

  function band_of(p: MockPerson): RosterBand {
    if (p.mode === "disabled") return "disabled";
    if (p.lastSeenMin === null) return "never_talked";
    return p.trust >= 110 ? "close" : "acquainted";
  }

  function recentOf(p: MockPerson): RosterRow["recent"] {
    const w = digestItems.find((d) => d.person.id === p.id);
    if (w) return w.activity;
    return p.lastSeenMin === null ? null : { kind: "messaged", at: iso(p.lastSeenMin) };
  }

  function summaryOf(ch: Chan): ChannelSummary {
    const { log: _log, ...rest } = ch;
    const unread = digestItems.filter((w) => w.channel === ch.id && w.activity.kind !== "commitment_due" && w.activity.kind !== "birthday").length;
    return { ...rest, unread };
  }

  const commands: Commands = {
    appStatus: () =>
      ok<AppStatus>({
        form: library ? "library" : "full",
        corpus: corpusLoaded ? { state: "loaded", name: "demo" } : { state: "missing" },
        needs_setup: settings.user_name === null && !introduced,
        locked,
        changed_elsewhere: changedElsewhere,
        save: openSave,
        missing_save: openSave === null ? params.get("missing") : null,
        home_dir: "/Users/demo/.dramatis",
        saves_dir: "/Users/demo/.dramatis/saves",
      }),
    appInfo: () => ok(info),
    updateStatus: () => ok(update),
    // Without `force` the mock asks nobody, as the engine does within its day.
    checkUpdate: (force) => {
      if (!force) return ok(update);
      if (params.get("update") === "fail") return fail({ code: "endpoint", detail: "offline" });
      update = found(update.auto);
      return ok(update);
    },
    setAutoUpdateCheck: (auto) => {
      update = { ...update, auto, asked: true };
      return ok(null);
    },
    // The preview opens links itself; the call exists so the app's path can be tested.
    openExternal: (url) => (/^https?:\/\//.test(url) ? ok(null) : fail({ code: "invalid", field: "url" })),
    setCorpus: (path) => {
      const p = path.trim().replace(/^["']|["']$/g, "");
      if (!p || p.includes("missing")) return fail({ code: "not_found" });
      if (p.includes("newer")) return fail({ code: "invalid", field: "corpus_version" });
      if (!p.endsWith(".folio")) return fail({ code: "invalid", field: "corpus" });
      corpusLoaded = true;
      return ok(null);
    },
    chooseCorpus: () => (params.get("dialog") === "1" ? ok<string | null>("/Users/demo/corpus/demo.folio") : fail({ code: "not_implemented" })),
    logOut: () => {
      openSave = null;
      locked = false;
      changedElsewhere = false;
      return ok(null);
    },
    setupEndpoint: (setup: EndpointSetup) => {
      const word = (v: string) => v.trim() !== "" && !/\s/.test(v.trim());
      if (!setup.name.trim()) return fail({ code: "invalid", field: "name" });
      if (!/^https?:\/\/\S+$/.test(setup.base_url.trim())) return fail({ code: "invalid", field: "base_url" });
      if (!word(setup.model)) return fail({ code: "invalid", field: "model" });
      const host = setup.host_model?.trim() || null;
      if (host && !word(host)) return fail({ code: "invalid", field: "host_model" });
      const name = setup.name.trim();
      const known = endpoints.profiles.find((p) => p.profile.name === name);
      const ids = new Set([...(known?.profile.model?.map((m) => m.id) ?? []), setup.model.trim(), ...(host ? [host] : [])]);
      const profile: Profile = { name, base_url: setup.base_url.trim(), wire_api: setup.wire_api, model: [...ids].map((id) => ({ id })) };
      const has_key = Boolean(setup.key?.trim()) || (known?.has_key ?? false);
      endpoints = {
        ...endpoints,
        profiles: [...endpoints.profiles.filter((p) => p.profile.name !== name), { profile, has_key }],
        roles: { chat: { profile: name, model: setup.model.trim() }, host: host ? { profile: name, model: host } : null, tts: endpoints.roles?.tts ?? null },
      };
      library = false;
      return ok(null);
    },
    wording: () => {
      const all = Object.values(localWording)[0] ?? {};
      // Mirrors the engine: English keeps the corpus's names and model-facing wording only.
      const keep = (k: string) => ["host.name", "org.name", "user.title", "lang.corpus"].includes(k) || /^(harness|tool|gender)\./.test(k);
      const entries = language === "english" ? Object.fromEntries(Object.entries(all).filter(([k]) => keep(k))) : all;
      return ok({ entries, scene_marker: scene });
    },
    interfaceLanguage: () => ok(language),
    setInterfaceLanguage: (next) => {
      language = next;
      return ok(null);
    },
    presence: async () => {
      const r = ok({ items: [...digestItems] });
      if (library || settings.user_name === null || band !== "open") return r;
      if (introPending) {
        introPending = false;
        greeted = true;
        void stream(
          "host",
          HOST,
          `${sc("A soft chime.")}\nHello, ${settings.user_name}. I look after things here. Everyone on the roster can be reached directly: open someone and talk.\n\nYou can also ask me to find who knows about something. Everyone starts **frozen**: they answer when you reach out and never start on their own. Enable someone if you'd like them to come to you now and then.`,
        );
      } else if (!greeted && digestItems.length) {
        greeted = true;
        void stream(
          "host",
          HOST,
          "Good morning. Mira finished looking into the east corridor for you, and Tess came by last night without saying why. It's also Juno's birthday today.",
          { actions: [{ tool: "digest" }] },
        );
      }
      return r;
    },
    coldstart: () =>
      ok(
        [
          ["mira", "Keeps the harbor archive; her own ledgers are the thickest on the roster."],
          ["oren", "On the east ward nearly every night the records mention."],
          ["tess", "Knows every route between the docks and the upper town."],
        ].map(([id, reason]) => ({ person: ref(id), mode: persons.get(id)!.mode, reason, citation: null })),
      ),
    clock: () => {
      const d = new Date();
      const pad = (n: number) => String(n).padStart(2, "0");
      return ok({
        in_world: { year: d.getFullYear() - 1000, month: d.getMonth() + 1, day: d.getDate() },
        local_time: `${pad(d.getHours())}:${pad(d.getMinutes())}`,
      });
    },
    digest: () => ok({ items: [...digestItems] }),
    roster: () => {
      const order: RosterBand[] = ["close", "acquainted", "never_talked", "disabled"];
      const all = [...persons.values()];
      const roster: Roster = {
        total: all.length,
        enabled: all.filter((p) => p.mode === "enabled").length,
        groups: order
          .map((band) => ({
            band,
            rows: all
              .filter((p) => band_of(p) === band)
              .sort((a, b) => b.trust - a.trust)
              .map((p) => ({ person: ref(p.id), mode: p.mode, recent: recentOf(p), companions: p.companions.map(ref), has_persona: true, talked: p.lastSeenMin !== null })),
          }))
          .filter((g) => g.rows.length > 0),
      };
      return ok(roster);
    },
    personPage: (id) => {
      const p = persons.get(id);
      if (!p) return fail({ code: "not_found" });
      return ok({
        person: ref(id),
        summary: p.summary,
        trust: p.trust,
        mode: p.mode,
        last_seen: p.lastSeenMin === null ? null : iso(p.lastSeenMin),
        companions: p.companions.map(ref),
        voice: p.voice,
        channel: channels.has(`dm:${id}`) ? `dm:${id}` : null,
        has_persona: true,
      });
    },
    findPeople: (topic) => ok(candidates(topic)),
    setMode: (target, mode: Mode) => {
      if (target.kind === "person") {
        const p = persons.get(target.id);
        if (!p) return fail({ code: "not_found" });
        p.mode = mode;
        const dm = channels.get(`dm:${target.id}`);
        if (dm) dm.mode = mode;
        setTimeout(() => {
          bus.modeChanged.emit({ target, mode });
          bus.rosterChanged.emit({ persons: [target.id] });
        });
      } else {
        const ch = channels.get(target.id);
        if (!ch) return fail({ code: "not_found" });
        ch.mode = mode;
        setTimeout(() => bus.modeChanged.emit({ target, mode }));
      }
      return ok(null);
    },
    channels: () => ok([...channels.values()].map(summaryOf)),
    history: (channel, before, limit) => {
      const ch = channels.get(channel);
      if (!ch) return fail({ code: "not_found" });
      const end = before ? ch.log.findIndex((m) => m.id === before) : ch.log.length;
      const stop = end < 0 ? ch.log.length : end;
      return ok(ch.log.slice(Math.max(0, stop - limit), stop));
    },
    openDirect: (id) => {
      const p = persons.get(id);
      if (!p) return fail({ code: "not_found" });
      if (p.mode === "disabled") return fail({ code: "disabled", person: id });
      const cid = direct(id);
      return ok(cid);
    },
    hostChannel: () => ok("host"),
    sendMessage: (channel, text, image) => {
      const ch = channels.get(channel);
      if (!ch) return fail({ code: "not_found" });
      if (!text.trim() && !image) return fail({ code: "invalid", field: "text" });
      const blocked = gate();
      if (blocked) return fail(blocked);
      if (away) {
        // Writing to anyone means he is back.
        away = false;
        setTimeout(() => bus.awayChanged.emit({ away: false, by_message: true }));
      }
      const m = msg(channel, USER, text, 0, image ? { attachment: { path: `data:${image.mime};base64,${image.data_base64}` } } : {});
      setTimeout(() => {
        bus.messageAdded.emit({ message: m });
        void (/\bfail\b/i.test(text) ? failTurn(ch) : ch.kind === "host" ? replyHost(text) : replyChannel(ch, text));
      });
      return ok(m.id);
    },
    rewind: (channel, message) => {
      const ch = channels.get(channel);
      const i = ch ? ch.log.findIndex((m) => m.id === message) : -1;
      if (!ch || i < 0) return fail({ code: "not_found" });
      const m = ch.log[i];
      if (m.author.kind !== "user") return fail({ code: "invalid", field: "request" });
      ch.log.splice(i);
      setTimeout(() => bus.rewound.emit({ channel, from: message }));
      return ok(m.text);
    },
    relay: (excerpt, _from, to) => {
      const p = persons.get(to);
      if (!p) return fail({ code: "not_found" });
      if (p.mode === "disabled") return fail({ code: "disabled", person: to });
      const blocked = gate();
      if (blocked) return fail(blocked);
      const cid = direct(to);
      const first = p.name.split(" ")[0];
      void stream(cid, person(to), `${sc(`${first} reads the quote twice.`)}\n"${excerpt.slice(0, 80)}"? That's not how I'd put it, but I can see where it comes from.`, { relayed: true });
      return ok(cid);
    },
    // The mock keeps no memories to write, so ending a conversation changes nothing.
    wrapup: (channel) => (channels.has(channel) ? ok(null) : fail({ code: "not_found" })),
    markRead: (channel) => {
      const before = digestItems.length;
      for (let i = digestItems.length - 1; i >= 0; i--) {
        const w = digestItems[i];
        if (w.channel === channel && w.activity.kind !== "commitment_due" && w.activity.kind !== "birthday") digestItems.splice(i, 1);
      }
      if (digestItems.length !== before) setTimeout(() => announceWorld({ kind: "digest" }));
      return ok(null);
    },
    createGroup: (members, topic) => {
      if (members.length === 0) return fail({ code: "invalid", field: "members" });
      const off = members.find((id) => persons.get(id)?.mode === "disabled");
      if (off) return fail({ code: "disabled", person: off });
      const id = nextId("g");
      addChannel({ id, kind: "group", origin: "user", participants: members.map(ref), topic: topic?.trim() || null, mode: "frozen" });
      setTimeout(() => announceWorld({ kind: "channel", id }));
      return ok(id);
    },
    openOffer: (message, action) => {
      const m = [...channels.values()].flatMap((c) => c.log).find((x) => x.id === message);
      const line = m?.actions[action];
      if (!m || !line) return fail({ code: "not_found" });
      if (line.tool !== "create_group") return fail({ code: "invalid", field: "action" });
      if (line.channel && channels.has(line.channel)) return ok(line.channel);
      // The same people already have a live group that is not archived, and the purpose is
      // none or is that group's name or topic (letters and digits only, case aside): that one.
      const key = (s: string | null | undefined) => (s ?? "").toLowerCase().replace(/[^\p{L}\p{N}]/gu, "");
      const same = [...channels.values()].find(
        (c) =>
          c.kind === "group" &&
          !c.archived &&
          c.participants.length === line.members.length &&
          line.members.every((p) => c.participants.some((x) => x.id === p.id)) &&
          (!line.purpose || (key(line.purpose) !== "" && [c.name, c.topic].some((t) => key(t) === key(line.purpose)))),
      );
      if (same) {
        m.actions[action] = { ...line, channel: same.id };
        return ok(same.id);
      }
      const id = nextId("g");
      const purpose = line.purpose ?? null;
      const made = addChannel({ id, kind: "group", origin: "user", participants: line.members, topic: purpose, mode: "frozen" });
      if (purpose) {
        // A group made for a purpose is named by it; one without stays unnamed.
        made.name = purpose;
        const by = m.author.kind === "person" ? (persons.get(m.author.id)?.name ?? "Someone") : "Host";
        msg(id, HOST, `${by} suggested this group: ${purpose}`, 0);
      }
      m.actions[action] = { ...line, channel: id };
      setTimeout(() => announceWorld({ kind: "channel", id }));
      return ok(id);
    },
    openFound: (message, action, person) => {
      const m = [...channels.values()].flatMap((c) => c.log).find((x) => x.id === message);
      const line = m?.actions[action];
      if (!m || !line) return fail({ code: "not_found" });
      if (line.tool !== "find_people" || !(line.persons ?? []).some((p) => p.id === person)) return fail({ code: "invalid", field: "action" });
      const p = persons.get(person);
      if (!p) return fail({ code: "not_found" });
      if (p.mode === "disabled") return fail({ code: "disabled", person });
      const cid = direct(person);
      const home = channels.get(cid);
      const by = m.author.kind === "person" ? (persons.get(m.author.id)?.name ?? "Someone") : "Host";
      const text = `${by} suggested you could help the user with: ${line.topic}`;
      if (home && !home.log.some((x) => x.service && x.text === text)) msg(cid, USER, text, 0, { service: true });
      return ok(cid);
    },
    addMembers: (channel, ids) => {
      const ch = channels.get(channel);
      if (ch?.kind !== "group") return fail({ code: "invalid", field: "request" });
      const off = ids.find((id) => persons.get(id)?.mode === "disabled");
      if (off) return fail({ code: "disabled", person: off });
      const added = ids.filter((id, i) => !ch.participants.some((p) => p.id === id) && ids.indexOf(id) === i);
      for (const id of added) {
        ch.participants = [...ch.participants, ref(id)];
        msg(channel, USER, `${ref(id).name} joins the conversation.`, 0, { service: true });
      }
      if (added.length) setTimeout(() => announceWorld({ kind: "channel", id: channel }));
      return ok(null);
    },
    removeMember: (channel, person) => {
      const ch = channels.get(channel);
      if (ch?.kind !== "group") return fail({ code: "invalid", field: "request" });
      if (!ch.participants.some((p) => p.id === person)) return ok(null);
      if (ch.participants.length <= 1) return fail({ code: "invalid", field: "request" });
      ch.participants = ch.participants.filter((p) => p.id !== person);
      msg(channel, USER, `${ref(person).name} leaves the conversation.`, 0, { service: true });
      setTimeout(() => announceWorld({ kind: "channel", id: channel }));
      return ok(null);
    },
    openInvite: (message, action, person) => {
      const m = [...channels.values()].flatMap((c) => c.log).find((x) => x.id === message);
      const line = m?.actions[action];
      if (!m || !line) return fail({ code: "not_found" });
      if (line.tool !== "invite_to_group" || !line.persons.some((p) => p.id === person)) return fail({ code: "invalid", field: "action" });
      const ch = channels.get(line.channel);
      if (ch?.kind !== "group") return fail({ code: "not_found" });
      if (persons.get(person)?.mode === "disabled") return fail({ code: "disabled", person });
      if (!ch.participants.some((p) => p.id === person)) {
        ch.participants = [...ch.participants, ref(person)];
        msg(line.channel, USER, `${ref(person).name} joins the conversation.`, 0, { service: true });
        setTimeout(() => announceWorld({ kind: "channel", id: line.channel }));
      }
      return ok(line.channel);
    },
    deleteGroup: (channel) => {
      if (channels.get(channel)?.kind !== "group") return fail({ code: "not_found" });
      channels.delete(channel);
      setTimeout(() => announceWorld({ kind: "channel", id: channel }));
      return ok(null);
    },
    renameChannel: (channel, name) => {
      const ch = channels.get(channel);
      if (!ch) return fail({ code: "not_found" });
      ch.name = name?.trim() || null;
      setTimeout(() => announceWorld({ kind: "channel", id: channel }));
      return ok(null);
    },
    setChannelAvatar: (channel, image) => {
      const ch = channels.get(channel);
      if (!ch) return fail({ code: "not_found" });
      ch.avatar = image ? { path: `data:${image.mime};base64,${image.data_base64}` } : null;
      setTimeout(() => announceWorld({ kind: "channel", id: channel }));
      return ok(ch.avatar);
    },
    archiveChannel: (channel, archived) => {
      const ch = channels.get(channel);
      if (!ch) return fail({ code: "not_found" });
      if (archived && ch.kind === "host") return fail({ code: "invalid", field: "channel" });
      ch.archived = archived;
      setTimeout(() => announceWorld({ kind: "channel", id: channel }));
      return ok(null);
    },
    facts: (filter: FactFilter) => {
      const all = [...facts.values()];
      const match = (f: FactView) => {
        switch (filter.scope) {
          case "all":
            return true;
          case "world":
            return !f.retracted && f.audience.kind === "world";
          case "own":
            return !f.retracted && f.audience.kind === "own";
          case "commitment":
            return !f.retracted && f.kind === "commitment";
          case "hurt":
            return !f.retracted && f.kind === "hurt";
          case "deleted":
            return f.retracted;
        }
      };
      const list: FactList = {
        total: all.filter((f) => !f.retracted).length,
        deleted: all.filter((f) => f.retracted).length,
        items: all.filter(match).sort((a, b) => b.created_at.localeCompare(a.created_at)),
      };
      return ok(list);
    },
    editFact: (id, text) => {
      const f = facts.get(id);
      if (!f) return fail({ code: "not_found" });
      if (!text.trim()) return fail({ code: "invalid", field: "text" });
      facts.set(id, { ...f, retracted: true });
      const { id: _old, created_at: _at, ...rest } = f;
      const next = addFact({ ...rest, author: USER, text, retracted: false });
      setTimeout(() => announceWorld({ kind: "fact", id }, { kind: "fact", id: next.id }));
      return ok(next.id);
    },
    deleteFact: (id) => toggleFact(id, true),
    restoreFact: (id) => toggleFact(id, false),
    corpusTemplates: () => ok([...new Set(PASSAGES.map((p) => p.template))].sort()),
    readUnit: (unit, around) => {
      const at = PASSAGES.findIndex((p) => p.citation.chunk_id === unit);
      if (at < 0) return fail({ code: "not_found" });
      return ok({
        unit: PASSAGES[at],
        before: PASSAGES.slice(Math.max(0, at - around), at),
        after: PASSAGES.slice(at + 1, at + 1 + around),
      });
    },
    searchCorpus: (query, only, kinds) => {
      const words = query.toLowerCase().split(/\s+/).filter(Boolean);
      const hits = PASSAGES.filter(
        (p) =>
          words.length > 0 &&
          words.some((w) => `${p.header} ${p.text}`.toLowerCase().includes(w)) &&
          (only.length === 0 || p.persons.some((q) => only.includes(q.id))) &&
          (kinds.length === 0 || kinds.includes(p.template)),
      );
      const named = [...persons.values()].find((p) => words.some((w) => p.name.toLowerCase().split(" ").includes(w)));
      const confidence: Confidence = hits.length >= 2 ? "high" : hits.length === 1 ? "medium" : "low";
      return ok({ hits, confidence, understood_as: named ? named.name : null });
    },
    resolveCitation: (c) => {
      const p = passage(c.chunk_id);
      if (!p) return ok({ status: "gone", passage: null, source_url: null, revision_url: null });
      const url = p.source_url;
      return ok({
        status: p.citation.revid === c.revid ? "current" : "updated",
        passage: p,
        source_url: url,
        revision_url: url ? `${url}?oldid=${c.revid}` : null,
      });
    },
    avatar: () => ok(null),
    hostAvatar: () => ok(null),
    userAvatar: () => ok(userAvatar),
    setUserAvatar: (image) => {
      userAvatar = image ? { path: `data:${image.mime};base64,${image.data_base64}` } : null;
      return ok(userAvatar);
    },
    ttsPlay: (_message): Result<MediaFile> => (tts ? ok({ path: silentWav() }) : fail({ code: "no_endpoint" })),
    ttsVoices: () => ok(tts ? ["alloy", "ember", "verse"] : []),
    setVoice: (id, voice) => {
      const p = persons.get(id);
      if (!p) return fail({ code: "not_found" });
      p.voice = voice;
      return ok(null);
    },
    quota: () => ok(quotaStatus()),
    setTier: (next) => {
      tier = next;
      setTimeout(() => bus.quotaChanged.emit({ status: quotaStatus() }));
      return ok(null);
    },
    away: () => ok(away),
    setAway: (next) => {
      away = next;
      setTimeout(() => bus.awayChanged.emit({ away, by_message: false }));
      return ok(null);
    },
    settings: () => ok({ ...settings }),
    dailyEvents: () =>
      ok(
        settings.events_per_day === 0
          ? []
          : [
              {
                day: iso().slice(0, 10),
                at: iso(),
                title: "Tea on the deck",
                text: "Mira brewed a pot for whoever passed by, and Oren stayed to argue about the water.",
                persons: [ref("mira"), ref("oren")],
                message: null,
              },
            ],
      ),
    updateSettings: (next) => {
      if (next.user_name !== null && !next.user_name.trim()) return fail({ code: "invalid", field: "user_name" });
      if (next.bio !== null && Array.from(next.bio.trim()).length > 200) return fail({ code: "invalid", field: "bio" });
      if (next.events_per_day > PACE[tier].events_max) return fail({ code: "invalid", field: "events_per_day" });
      settings = { ...next, bio: next.bio?.trim() || null };
      introduced = true;
      return ok(null);
    },
    storageUsage: () => ok(storageUsage()),
    clearStorage: (categories) => {
      if (categories.some((k) => k !== "avatars" && k !== "audio")) return fail({ code: "invalid", field: "categories" });
      for (const k of categories) cache[k as "avatars" | "audio"] = { kb: 0, files: 0 };
      return ok(storageUsage());
    },
    saves: () => ok(saves.map((s) => ({ ...s, current: s.name === openSave }))),
    newSave: (name) => {
      const bad = saveNameError(name);
      if (bad) return fail(bad);
      saves.push({ name, size_kb: 96, played_at: iso(), current: false, encrypted: false });
      known.add(name);
      return ok(null);
    },
    switchSave: (name) => {
      const target = saves.find((s) => s.name === name);
      if (!target) return fail({ code: "not_found" });
      if (target.encrypted && !known.has(name)) return fail({ code: "locked" });
      openSave = name;
      changedElsewhere = false;
      return ok(null);
    },
    unlock: (name, password) => {
      const target = saves.find((s) => s.name === name);
      if (!target) return fail({ code: "not_found" });
      if (target.encrypted && passwords.get(name) !== password) return fail({ code: "wrong_password" });
      known.add(name);
      if (name === openSave) locked = false;
      return ok(null);
    },
    setPassword: (old, next) => {
      const current = saves.find((s) => s.name === openSave);
      if (!current) return fail({ code: "not_found" });
      if (locked) return fail({ code: "locked" });
      if (old === null && next === null) return fail({ code: "invalid", field: "password" });
      if (current.encrypted !== (old !== null)) return fail({ code: "invalid", field: "old" });
      if (old !== null && passwords.get(current.name) !== old) return fail({ code: "wrong_password" });
      if (next === "") return fail({ code: "invalid", field: "new" });
      current.encrypted = next !== null;
      if (next === null) passwords.delete(current.name);
      else passwords.set(current.name, next);
      return ok(null);
    },
    copySave: (from, to) => {
      const src = saves.find((s) => s.name === from);
      if (!src) return fail({ code: "not_found" });
      const bad = saveNameError(to);
      if (bad) return fail(bad);
      saves.push({ ...src, name: to, played_at: iso(), current: false });
      if (passwords.has(from)) passwords.set(to, passwords.get(from) as string);
      if (known.has(from)) known.add(to);
      return ok(null);
    },
    renameSave: (from, to) => {
      const src = saves.find((s) => s.name === from);
      if (!src) return fail({ code: "not_found" });
      const bad = saveNameError(to);
      if (bad) return fail(bad);
      src.name = to;
      if (passwords.has(from)) passwords.set(to, passwords.get(from) as string);
      passwords.delete(from);
      if (known.delete(from)) known.add(to);
      if (openSave === from) openSave = to;
      return ok(null);
    },
    deleteSave: (name) => {
      if (name === openSave) return fail({ code: "invalid", field: "name" });
      if (!saves.some((s) => s.name === name)) return fail({ code: "not_found" });
      saves = saves.filter((s) => s.name !== name);
      return ok(null);
    },
    exportSave: (name, path) => {
      if (!saves.some((s) => s.name === name)) return fail({ code: "not_found" });
      return path.trim() ? ok(null) : fail({ code: "invalid", field: "path" });
    },
    importSave: (path, name, password) => {
      const used = name ?? path.split(/[\\/]/).pop()?.replace(/\.[^.]*$/, "") ?? "";
      const bad = saveNameError(used);
      if (bad) return fail(bad);
      // A file with "vault" in its name stands for an encrypted export ("hunter2").
      const encrypted = path.includes("vault");
      if (encrypted && password === null) return fail({ code: "locked" });
      if (encrypted && password !== "hunter2") return fail({ code: "wrong_password" });
      saves.push({ name: used, size_kb: 128, played_at: iso(), current: false, encrypted });
      known.add(used);
      if (encrypted) passwords.set(used, "hunter2");
      return ok(used);
    },
    endpoints: () => ok(structuredClone(endpoints)),
    addProfile: (profile) => {
      if (!profile.name.trim()) return fail({ code: "invalid", field: "name" });
      if (endpoints.profiles.some((p) => p.profile.name === profile.name)) return fail({ code: "invalid", field: "name" });
      endpoints = { ...endpoints, profiles: [...endpoints.profiles, { profile, has_key: false }] };
      return ok(null);
    },
    updateProfile: (name, profile) => {
      endpoints = { ...endpoints, profiles: endpoints.profiles.map((p) => (p.profile.name === name ? { ...p, profile } : p)) };
      return ok(null);
    },
    removeProfile: (name) => {
      const roles = endpoints.roles;
      endpoints = {
        ...endpoints,
        profiles: endpoints.profiles.filter((p) => p.profile.name !== name),
        roles: roles && roles.chat.profile === name ? null : roles && roles.tts?.profile === name ? { ...roles, tts: null } : roles,
      };
      if (!endpoints.roles) library = true;
      return ok(null);
    },
    fetchModels: (name) => {
      const p = endpoints.profiles.find((x) => x.profile.name === name);
      if (!p) return fail({ code: "not_found" });
      if (!p.has_key && !p.profile.base_url.startsWith("http://127.0.0.1")) {
        return fail({ code: "endpoint", detail: '401 Unauthorized: {"error":{"message":"Authentication failed: missing or invalid API key","type":"authentication_error"}}' });
      }
      if (name === "primary") return ok(["model-a", "model-b", "tts-1"]);
      return ok(p.profile.base_url.includes("deepseek") ? ["deepseek-chat", "deepseek-reasoner"] : ["local-model"]);
    },
    setKey: (name, key) => {
      if (!key.trim()) return fail({ code: "invalid", field: "key" });
      // The key is dropped on the floor: the mock has no keychain, and nothing returns it.
      endpoints = { ...endpoints, profiles: endpoints.profiles.map((p) => (p.profile.name === name ? { ...p, has_key: true } : p)) };
      return ok(null);
    },
    setRole: (kind, role) => {
      if (kind === "chat") {
        if (!role) return fail({ code: "invalid", field: "role" });
        endpoints = { ...endpoints, roles: { ...(endpoints.roles ?? {}), chat: role } };
        library = false;
      } else if (kind === "host") {
        if (!endpoints.roles) return fail({ code: "invalid", field: "role" });
        endpoints = { ...endpoints, roles: { ...endpoints.roles, host: role } };
      } else {
        if (!endpoints.roles) return fail({ code: "invalid", field: "role" });
        endpoints = { ...endpoints, roles: { ...endpoints.roles, tts: role } };
        tts = role !== null;
      }
      return ok(null);
    },
  };

  function toggleFact(id: FactId, retracted: boolean): Result<null> {
    const f = facts.get(id);
    if (!f) return fail({ code: "not_found" });
    facts.set(id, { ...f, retracted });
    const changes: { kind: "fact"; id: FactId }[] = [{ kind: "fact", id }];
    if (f.kind === "hurt" && f.about?.kind === "person") {
      const p = persons.get(f.about.id);
      if (p) p.trust += retracted ? 10 : -10;
      const pid = f.about.id;
      setTimeout(() => bus.worldChanged.emit({ changes: [...changes, { kind: "trust", person: pid }] }));
    } else {
      setTimeout(() => bus.worldChanged.emit({ changes }));
    }
    return ok(null);
  }

  return { commands, events: bus };
}
