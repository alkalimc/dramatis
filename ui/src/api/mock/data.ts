// Invented demo content for the mock api. Nothing here comes from any corpus.
import type { Citation, Mode, Passage } from "../../api.gen";

export interface MockPerson {
  id: string;
  name: string;
  summary: string;
  trust: number;
  mode: Mode;
  /** Minutes since the last message with the user; `null` = never talked. */
  lastSeenMin: number | null;
  tasksDone: number;
  companions: string[];
  voice: string | null;
  /** Topics find_people matches, with the reason line it shows. */
  knows: [RegExp, string][];
}

export const PERSONS: MockPerson[] = [
  {
    id: "mira",
    name: "Mira Vale",
    summary: "Archivist of the harbor records · keeps the tide ledgers",
    trust: 137,
    mode: "enabled",
    lastSeenMin: 60 * 20,
    tasksDone: 4,
    companions: ["oren", "tess"],
    voice: "verse",
    knows: [
      [/harbor|ledger|tide|archive|record|flood|east/i, "Her own ledgers cover the harbor flood years."],
      [/.*/, "Keeps the index to most of the archive."],
    ],
  },
  {
    id: "oren",
    name: "Oren Lark",
    summary: "Night-shift medic at the east ward",
    trust: 112,
    mode: "frozen",
    lastSeenMin: 60 * 9,
    tasksDone: 1,
    companions: ["mira", "iris"],
    voice: null,
    knows: [
      [/ward|shift|medic|night|rota|east/i, "He wrote most of the east ward's night notes."],
      [/flood/i, "He treated people during the second flood."],
    ],
  },
  {
    id: "tess",
    name: "Tess Arden",
    summary: "Courier between the upper town and the docks",
    trust: 104,
    mode: "frozen",
    lastSeenMin: 60 * 24 * 3,
    tasksDone: 2,
    companions: ["mira", "kael"],
    voice: null,
    knows: [
      [/dock|courier|route|upper|town|bridge/i, "She has run the dock routes for years."],
    ],
  },
  {
    id: "juno",
    name: "Juno Reyes",
    summary: "Engineer on the tide gates",
    trust: 100,
    mode: "frozen",
    lastSeenMin: 60 * 24 * 12,
    tasksDone: 0,
    companions: ["kael"],
    voice: null,
    knows: [[/gate|tide|engine|flood|repair/i, "Her own reports describe the gate repairs."]],
  },
  {
    id: "kael",
    name: "Kael Moss",
    summary: "Quartermaster · counts everything twice",
    trust: 100,
    mode: "frozen",
    lastSeenMin: null,
    tasksDone: 0,
    companions: ["juno", "tess"],
    voice: null,
    knows: [[/supply|stores|count|ration|dock/i, "The supply lists are in his hand."]],
  },
  {
    id: "iris",
    name: "Iris Penn",
    summary: "Records the council's sessions",
    trust: 100,
    mode: "frozen",
    lastSeenMin: null,
    tasksDone: 0,
    companions: ["oren"],
    voice: null,
    knows: [[/council|vote|session|law|decree/i, "She took the minutes of those sessions."]],
  },
  {
    id: "bram",
    name: "Bram Hollis",
    summary: "Lighthouse keeper on the outer rock",
    trust: 100,
    mode: "frozen",
    lastSeenMin: null,
    tasksDone: 0,
    companions: [],
    voice: null,
    knows: [[/light|storm|ship|sea|night/i, "He logged every ship that passed in the storms."]],
  },
  {
    id: "nell",
    name: "Nell Ashby",
    summary: "Apprentice cartographer",
    trust: 100,
    mode: "frozen",
    lastSeenMin: null,
    tasksDone: 0,
    companions: ["tess"],
    voice: null,
    knows: [[/map|chart|street|route|bridge/i, "She redrew the street charts last spring."]],
  },
  {
    id: "silas",
    name: "Silas Crane",
    summary: "Former harbor master, retired",
    trust: 88,
    mode: "disabled",
    lastSeenMin: 60 * 24 * 40,
    tasksDone: 0,
    companions: ["mira"],
    voice: null,
    knows: [[/harbor|flood/i, "He ran the harbor during the first flood."]],
  },
];

function cite(page: string, revid: number, chunk_id: string): Citation {
  return { page, revid, span_from: 0, span_to: 400, chunk_id };
}

export const PASSAGES: Passage[] = [
  {
    citation: cite("Harbor Ledger/Flood Years", 1041, "c-ledger-1"),
    template: "profile",
    header: "Harbor Ledger · Flood Years",
    text: "The east corridor was sealed in the first month after the tide gates failed. The archive moved its ledgers to the upper floor the same night.",
    persons: [{ id: "mira", name: "Mira Vale" }],
    source_url: "https://example.invalid/wiki/Harbor_Ledger",
  },
  {
    citation: cite("East Ward/Night Notes", 2203, "c-ward-1"),
    template: "story",
    header: "East Ward · Night Notes",
    text: "It was high summer when the corridor closed; the heat in the ward was the worst we had seen, and the water came in with the evening tide.",
    persons: [{ id: "oren", name: "Oren Lark" }],
    source_url: "https://example.invalid/wiki/East_Ward",
  },
  {
    citation: cite("Tide Gates/Repair Report", 877, "c-gates-1"),
    template: "profile",
    header: "Tide Gates · Repair Report",
    text: "The second gate was rebuilt with the stone from the old bridge. The repair took eleven weeks and three crews.",
    persons: [{ id: "juno", name: "Juno Reyes" }],
    source_url: "https://example.invalid/wiki/Tide_Gates",
  },
  {
    citation: cite("Dock Routes/Upper Town", 3310, "c-docks-1"),
    template: "voice",
    header: "Dock Routes · Upper Town",
    text: "Take the stairs by the fish market, never the bridge after dark. The bridge belongs to the fog after sunset.",
    persons: [{ id: "tess", name: "Tess Arden" }],
    source_url: "https://example.invalid/wiki/Dock_Routes",
  },
  {
    citation: cite("Council/Session Minutes", 1502, "c-council-1"),
    template: "story",
    header: "Council · Session Minutes",
    text: "The council voted to move the ward uphill. Two members abstained, citing the cost of the new road.",
    persons: [
      { id: "iris", name: "Iris Penn" },
      { id: "oren", name: "Oren Lark" },
    ],
    source_url: null,
  },
];

export function passage(chunkId: string): Passage | undefined {
  return PASSAGES.find((p) => p.citation.chunk_id === chunkId);
}
