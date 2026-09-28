// In-memory stand-in for the engine, behind the same signatures as the generated
// commands. Used outside the Tauri webview (browser preview, tests). Replies are canned
// and streamed through the same events the engine sends.
//
// Demo switches (query string): `form=library` (no endpoint yet), `first=1` (first open),
// `tts=0` (no read-aloud role), `tier=ultra`, `band=quiet|exhausted`, `marker=paren`
// (scene lines in `(...)` instead of `*...*`).
import type {
  ApiError,
  AppStatus,
  Author,
  CaseFile,
  ChannelId,
  ChannelSummary,
  Citation,
  Confidence,
  EndpointsView,
  FactFilter,
  FactId,
  FactList,
  FactView,
  MediaFile,
  Message,
  MessageRef,
  Mode,
  PersonId,
  PersonRef,
  Profile,
  QuotaBand,
  QuotaStatus,
  Roster,
  RosterBand,
  RosterRow,
  Settings,
  TaskSummary,
  Tier,
  ToolAction,
  Waiting,
} from "../../api.gen";
import type { Commands, Events } from "..";
import { createBus } from "./bus";
import { PASSAGES, PERSONS, passage, type MockPerson } from "./data";

type Result<T> = Promise<{ status: "ok"; data: T } | { status: "error"; error: ApiError }>;

const ok = <T>(data: T): Result<T> => Promise.resolve({ status: "ok", data });
const fail = <T>(error: ApiError): Result<T> => Promise.resolve({ status: "error", error });

const iso = (minutesAgo = 0) => new Date(Date.now() - minutesAgo * 60_000).toISOString();
const later = (minutes: number) => iso(-minutes);

const PRESETS: Profile[] = [
  { name: "deepseek", base_url: "https://api.deepseek.com", wire_api: "chat", model: [] },
  { name: "openai-compatible", base_url: "https://api.openai.com/v1", wire_api: "chat", model: [] },
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
  const ref = (id: PersonId): PersonRef => ({ id, name: persons.get(id)?.name ?? id });

  let library = params.get("form") === "library";
  let tts = params.get("tts") !== "0";
  let settings: Settings = {
    user_name: params.get("first") === "1" ? null : "Rowan",
    birthday: null,
    quiet_hours: { from: "23:00", to: "07:00" },
    notifications: true,
  };
  let introPending = settings.user_name === null;
  let greeted = false;
  let tier: Tier = params.get("tier") === "ultra" ? "ultra" : "middle";
  const band: QuotaBand =
    params.get("band") === "quiet" ? "quiet" : params.get("band") === "exhausted" ? "exhausted" : "open";

  let seq = 0;
  const nextId = (prefix: string) => `${prefix}-${++seq}`;

  interface Chan extends ChannelSummary {
    log: Message[];
  }
  const channels = new Map<ChannelId, Chan>();
  const addChannel = (c: Omit<Chan, "log" | "last_at">) => {
    const ch: Chan = { ...c, last_at: null, log: [] };
    channels.set(c.id, ch);
    return ch;
  };
  const msg = (channel: ChannelId, author: Author, text: string, minutesAgo: number, extra: Partial<Message> = {}): Message => {
    const m: Message = {
      id: nextId("m"),
      channel,
      author,
      text,
      actions: [],
      attachment: null,
      cites: [],
      confidence: null,
      relayed: false,
      task: null,
      note: null,
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

  // ---- tasks and facts ----
  const tasks = new Map<string, TaskSummary>();
  const addTask = (t: Omit<TaskSummary, "created_at">, minutesAgo: number) => {
    const task = { ...t, created_at: iso(minutesAgo) };
    tasks.set(t.id, task);
    return task;
  };
  const facts = new Map<FactId, FactView>();
  const addFact = (f: Omit<FactView, "id" | "created_at" | "delivered" | "retracted"> & Partial<FactView>, minutesAgo = 0) => {
    const fact: FactView = { id: nextId("f"), delivered: false, retracted: false, created_at: iso(minutesAgo), ...f };
    facts.set(fact.id, fact);
    return fact;
  };
  const reactions: MessageRef[] = [];

  if (!introPending) {
    msg("host", USER, "Who knows about the harbor flood? Ask someone for me.", 60 * 26);
    msg(
      "host",
      HOST,
      "Mira Vale keeps the flood ledgers, so I asked her. Her answer will land in your conversation with her.",
      60 * 26 - 1,
      {
        actions: [
          { tool: "find_people", topic: "harbor flood" },
          { tool: "ask", person: ref("mira"), question: "How did the east corridor come to be sealed?" },
        ],
      },
    );

    addTask({ id: "t-case", question: "What really happened in the east corridor?", assignee: null, parent: null, status: "active", pinned: true, channel: null }, 60 * 30);
    addTask({ id: "t-mira", question: "How did the east corridor come to be sealed?", assignee: ref("mira"), parent: "t-case", status: "done", pinned: false, channel: direct("mira") }, 60 * 26);
    addTask({ id: "t-oren", question: "When was the east corridor sealed?", assignee: ref("oren"), parent: "t-case", status: "active", pinned: false, channel: direct("oren") }, 60 * 5);
    addTask({ id: "t-tess", question: "Which route is safest after dark?", assignee: ref("tess"), parent: null, status: "done", pinned: false, channel: direct("tess") }, 60 * 50);

    msg(direct("mira"), USER, "Morning. Busy day at the archive?", 60 * 48);
    msg(direct("mira"), person("mira"), `${sc("Mira sets down a stack of ledgers.")}\nAlways. The tide books don't file themselves.`, 60 * 48 - 2);
    msg(
      direct("mira"),
      person("mira"),
      `${sc("She runs a finger down the index.")}\nThe east corridor was sealed in the **first month** after the gates failed. My own ledger says so.\nOren remembers that night differently: he says it was high summer.\n\nI wrote it up for you.`,
      60 * 20,
      {
        actions: [
          { tool: "search", query: "east corridor sealed flood" },
          { tool: "request_join", person: ref("oren"), topic: null },
          { tool: "report" },
        ],
        cites: [cite("c-ledger-1"), cite("c-ward-1", 2201)],
        task: "t-mira",
        note: { path: "data:text/plain;charset=utf-8,East%20corridor%3A%20sealed%20in%20the%20first%20month%20(ledger)%3B%20Oren%20recalls%20summer." },
      },
    );

    msg(direct("oren"), person("oren"), "You said you'd bring the new rota by today. No rush, just reminding you.", 60 * 9);
    msg(direct("oren"), USER, "Mira says the corridor closed in the first month. What do you remember?", 60 * 8);
    const orenReaction = msg(
      direct("oren"),
      person("oren"),
      `${sc("Oren frowns.")}\nIf Mira wrote that down, she has the ledger. But I was on that ward, and it was hot. I remember it differently: the evacuation order came late, not the warning.`,
      60 * 8 - 1,
      { relayed: true },
    );
    reactions.push({ channel: orenReaction.channel, message: orenReaction.id, author: orenReaction.author, excerpt: "I remember it differently: the evacuation order came late, not the warning." });

    msg(direct("tess"), USER, "Which route is safest after dark?", 60 * 50);
    msg(direct("tess"), person("tess"), "Take the stairs by the fish market, never the bridge after dark.", 60 * 49, {
      actions: [{ tool: "search", query: "dock routes after dark" }, { tool: "report" }],
      cites: [cite("c-docks-1")],
      task: "t-tess",
    });

    msg(group.id, USER, "Did the rota change today?", 60 * 3);
    msg(group.id, person("oren"), "It did. I'm on nights this week.", 60 * 3 - 1);
    msg(group.id, person("tess"), sc("Tess drops a parcel on the table."), 60 * 3 - 2);
    msg(group.id, person("tess"), "I carried the new rota up this morning. Kael counted the copies twice.", 60 * 3 - 2);
    msg(group.id, USER, "Tess, who built the old bridge?", 60 * 2);
    msg(group.id, person("tess"), "The old bridge? I don't know much about how it was built. Juno might.", 60 * 2 - 1, {
      actions: [{ tool: "search", query: "old bridge builder" }],
      confidence: "low",
    });

    addFact({ author: person("oren"), audience: { kind: "participants", channel: group.id, persons: group.participants }, kind: "fact", about: null, due: null, text: "Oren mentioned the east ward changed its rota this week." }, 60 * 3);
    addFact({ author: USER, audience: { kind: "participants", channel: direct("oren"), persons: [ref("oren")] }, kind: "commitment", about: { kind: "person", id: "oren" }, due: iso(60), text: "You said you'd bring Oren the new rota today." }, 60 * 30);
    addFact({ author: USER, audience: { kind: "participants", channel: direct("mira"), persons: [ref("mira")] }, kind: "hurt", about: { kind: "person", id: "mira" }, due: null, text: "Your ledgers are useless, nobody reads them." }, 60 * 47);
    addFact({ author: USER, audience: { kind: "participants", channel: direct("mira"), persons: [ref("mira")] }, kind: "fact", about: { kind: "user" }, due: null, text: "You said you don't like sweets.", retracted: true }, 60 * 46);
    addFact({ author: person("mira"), audience: { kind: "own", person: ref("mira") }, kind: "fact", about: { kind: "user" }, due: null, text: "Mira noticed you prefer short answers." }, 60 * 45);
    addFact({ author: USER, audience: { kind: "world" }, kind: "conclusion", about: { kind: "task", id: "t-case" }, due: null, text: "The warning came late, so the corridor closed before people got out." }, 60 * 7);
  }

  const digestItems: Waiting[] = introPending
    ? []
    : [
        { person: ref("mira"), channel: direct("mira"), activity: { kind: "replied", task: "t-mira", question: tasks.get("t-mira")!.question, at: iso(60 * 20) } },
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
    await wait(300);
    for (const action of extra.actions ?? []) {
      bus.toolCalled.emit({ channel, message: id, author, action });
      await wait(450);
    }
    for (let i = 0; i < text.length; i += 14) {
      bus.messageDelta.emit({ channel, message: id, author, delta: text.slice(i, i + 14) });
      await wait(40);
    }
    const m = msg(channel, author, text, 0, extra);
    // `msg` minted its own id; keep the streamed one stable.
    const ch = channels.get(channel)!;
    ch.log[ch.log.length - 1] = { ...m, id };
    bus.messageAdded.emit({ message: { ...m, id } });
    return id;
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
      actions.push({ tool: "find_people", topic: text.slice(0, 60) });
      reply = found.length
        ? `These could help:\n\n${found.map((c) => `- **${c.person.name}**: ${c.reason}`).join("\n")}\n\nWant me to ask one of them?`
        : "Nobody on the roster has material on that.";
    } else if (/\bnew\b|\bwhat's up\b/i.test(text)) {
      actions.push({ tool: "digest" });
      reply = digestItems.length ? "Mira answered your question and Oren is waiting on the rota." : "Nothing new.";
    } else {
      reply = "I'm here. Ask me to find someone, to ask someone something for you, or to note something down.";
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

  function quotaStatus(): QuotaStatus {
    if (tier === "ultra") return { tier, band: "open", windows: [], binding: null, spent: spend() };
    const used = band === "open" ? [0.42, 0.18] : band === "quiet" ? [0.86, 0.21] : [1.0, 0.44];
    return {
      tier,
      band,
      windows: [
        { span: "five_hours", used: used[0], release_at: band === "open" ? null : later(band === "quiet" ? 50 : 125) },
        { span: "seven_days", used: used[1], release_at: null },
      ],
      binding: band === "open" ? null : "five_hours",
      spent: spend(),
    };
  }
  function spend() {
    return [
      { shape: "direct" as const, points: 41000 },
      { shape: "group" as const, points: 18500 },
      { shape: "ask" as const, points: 22000 },
      { shape: "host" as const, points: 9000 },
      { shape: "opening" as const, points: 3000 },
      { shape: "wrapup" as const, points: 4200 },
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
    return rest;
  }

  const commands: Commands = {
    appStatus: () =>
      ok<AppStatus>({
        form: library ? "library" : "full",
        corpus: { state: "loaded", name: "demo", encoder_mismatch: false },
        needs_setup: settings.user_name === null,
      }),
    wording: () => ok({ entries: {}, scene_marker: scene }),
    presence: async () => {
      const r = ok({ items: [...digestItems] });
      if (library || settings.user_name === null || band !== "open") return r;
      if (introPending) {
        introPending = false;
        greeted = true;
        void stream(
          "host",
          HOST,
          `${sc("A soft chime.")}\nHello, ${settings.user_name}. I look after things here. Everyone on the roster can be reached directly: open someone and talk.\n\nYou can also ask me to find who knows about something, or to have someone look into a question for you. Everyone starts **frozen**: they answer when you reach out and never start on their own. Enable someone if you'd like them to come to you now and then.`,
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
              .map((p) => ({ person: ref(p.id), mode: p.mode, recent: recentOf(p), companions: p.companions.map(ref) })),
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
        tasks_done: p.tasksDone,
        companions: p.companions.map(ref),
        voice: p.voice,
        channel: channels.has(`dm:${id}`) ? `dm:${id}` : null,
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
      const m = msg(channel, USER, text, 0, image ? { attachment: { path: `data:${image.mime};base64,${image.data_base64}` } } : {});
      setTimeout(() => {
        bus.messageAdded.emit({ message: m });
        void (ch.kind === "host" ? replyHost(text) : replyChannel(ch, text));
      });
      return ok(m.id);
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
    deleteGroup: (channel) => {
      if (channels.get(channel)?.kind !== "group") return fail({ code: "not_found" });
      channels.delete(channel);
      setTimeout(() => announceWorld({ kind: "channel", id: channel }));
      return ok(null);
    },
    ask: (id, question, _turns, parent) => {
      const p = persons.get(id);
      if (!p) return fail({ code: "not_found" });
      if (p.mode === "disabled") return fail({ code: "disabled", person: id });
      if (!question.trim()) return fail({ code: "invalid", field: "question" });
      const blocked = gate();
      if (blocked) return fail(blocked);
      const cid = direct(id);
      const task = addTask({ id: nextId("t"), question, assignee: ref(id), parent, status: "active", pinned: false, channel: cid }, 0);
      setTimeout(async () => {
        announceWorld({ kind: "task", id: task.id });
        const hit = PASSAGES.find((x) => x.persons.some((q) => q.id === id));
        const other = p.companions.find((c) => persons.get(c)?.mode !== "disabled");
        const found = hit && knows(p, question);
        const actions: ToolAction[] = [{ tool: "search", query: question.slice(0, 48) }];
        if (!found && other) actions.push({ tool: "request_join", person: ref(other), topic: null });
        actions.push({ tool: "report" });
        const text = found
          ? `${sc(`${p.name.split(" ")[0]} comes back with a folder.`)}\nHere's what I found: ${hit.text}`
          : "I looked, but I couldn't find anything solid on this. Someone else may know more.";
        const extra: Partial<Message> = found ? { actions, cites: [hit.citation], task: task.id } : { actions, confidence: "low", task: task.id };
        await stream(cid, person(id), text, extra);
        tasks.set(task.id, { ...task, status: "done" });
        digestItems.unshift({ person: ref(id), channel: cid, activity: { kind: "replied", task: task.id, question, at: iso() } });
        announceWorld({ kind: "task", id: task.id }, { kind: "digest" });
      });
      return ok(task.id);
    },
    tasks: () => ok([...tasks.values()].sort((a, b) => b.created_at.localeCompare(a.created_at))),
    caseFile: (id) => {
      const task = tasks.get(id);
      if (!task) return fail({ code: "not_found" });
      const replyOf = (t: TaskSummary) => {
        const ch = t.channel ? channels.get(t.channel) : undefined;
        return ch?.log.find((m) => m.task === t.id) ?? null;
      };
      const conclusion = [...facts.values()].find((f) => f.kind === "conclusion" && !f.retracted && f.about?.kind === "task" && f.about.id === id) ?? null;
      const cf: CaseFile = {
        task,
        trail: [...tasks.values()].filter((t) => t.parent === id).map((t) => ({ task: t, reply: replyOf(t) })),
        conclusion,
        reactions: id === "t-case" ? reactions : [],
      };
      return ok(cf);
    },
    openCase: (question) => {
      if (!question.trim()) return fail({ code: "invalid", field: "question" });
      const t = addTask({ id: nextId("t"), question, assignee: null, parent: null, status: "active", pinned: true, channel: null }, 0);
      setTimeout(() => announceWorld({ kind: "task", id: t.id }));
      return ok(t.id);
    },
    pinTask: (id) => {
      const t = tasks.get(id);
      if (!t) return fail({ code: "not_found" });
      tasks.set(id, { ...t, pinned: true });
      setTimeout(() => announceWorld({ kind: "task", id }));
      return ok(null);
    },
    unpinTask: (id) => {
      const t = tasks.get(id);
      if (!t) return fail({ code: "not_found" });
      tasks.set(id, { ...t, pinned: false });
      setTimeout(() => announceWorld({ kind: "task", id }));
      return ok(null);
    },
    writeConclusion: (id, text) => {
      if (!tasks.has(id)) return fail({ code: "not_found" });
      if (!text.trim()) return fail({ code: "invalid", field: "text" });
      for (const f of facts.values()) {
        if (f.kind === "conclusion" && f.about?.kind === "task" && f.about.id === id && !f.retracted) facts.set(f.id, { ...f, retracted: true });
      }
      const f = addFact({ author: USER, audience: { kind: "world" }, kind: "conclusion", about: { kind: "task", id }, due: null, text });
      setTimeout(() => announceWorld({ kind: "fact", id: f.id }, { kind: "task", id }));
      return ok(f.id);
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
    searchCorpus: (query, only) => {
      const words = query.toLowerCase().split(/\s+/).filter(Boolean);
      const hits = PASSAGES.filter(
        (p) =>
          words.length > 0 &&
          words.some((w) => `${p.header} ${p.text}`.toLowerCase().includes(w)) &&
          (only.length === 0 || p.persons.some((q) => only.includes(q.id))),
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
    settings: () => ok({ ...settings }),
    updateSettings: (next) => {
      if (next.user_name !== null && !next.user_name.trim()) return fail({ code: "invalid", field: "user_name" });
      settings = { ...next };
      return ok(null);
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
