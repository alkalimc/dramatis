import type {
  ActivityFailure,
  AppStatus,
  Author,
  Candidate,
  ChannelId,
  ChannelSummary,
  Clock,
  Digest,
  EndpointsView,
  Failure,
  Message,
  MessageAdded,
  MessageDelta,
  MessageId,
  ModeChanged,
  PersonId,
  PersonRef,
  QuotaStatus,
  Rewound,
  Roster,
  Settings,
  TaskId,
  ToolAction,
  ToolCalled,
  TurnActivity,
  WorldChanged,
  Wording,
} from "../api";
import { makeT, type T } from "../i18n";

export type Drawer =
  | { kind: "roster" }
  | { kind: "person"; id: PersonId }
  | { kind: "cases" }
  | { kind: "case"; id: TaskId }
  | { kind: "world" }
  | { kind: "system"; tab: "quota" | "endpoints" | "saves" };

/** What the ask card starts with; every field optional. */
export interface AskDraft {
  question?: string;
  person?: PersonId;
  parent?: TaskId;
}

/** A reply still being written: text so far and the actions it has taken. */
export interface Stream {
  channel: ChannelId;
  author: Author;
  text: string;
}

/** Someone generating in a channel: looking something up, or writing/waiting. */
export interface Typing {
  author: Author;
  tool: boolean;
}

/** Why the last turn in a channel ended without a reply. */
export interface TurnFailure {
  author: Author;
  reason: ActivityFailure;
}

/** Key of a speaker within one channel's typing map. */
export function typingKey(a: Author): string {
  return a.kind === "person" ? a.id : a.kind;
}

/**
 * Everything the store loads from the engine, each with its own load state and error, so
 * the screen that shows it can say what failed and offer to retry. `events` is the live
 * subscription; `history:<channel>` one channel's log.
 */
export type LoadKey =
  | "status"
  | "wording"
  | "settings"
  | "endpoints"
  | "digest"
  | "coldstart"
  | "roster"
  | "quota"
  | "channels"
  | "host"
  | "events"
  | `history:${ChannelId}`;

export interface State {
  status: AppStatus | null;
  wording: Wording;
  t: T;
  clock: Clock | null;
  settings: Settings | null;
  endpoints: EndpointsView | null;
  digest: Digest;
  coldstart: Candidate[];
  roster: Roster | null;
  quota: QuotaStatus | null;
  hostChannel: ChannelId | null;
  channels: ChannelSummary[];
  /** Per channel, oldest first. */
  messages: Record<ChannelId, Message[]>;
  /** Channels whose oldest page has been reached. */
  historyDone: Record<ChannelId, boolean>;
  streams: Record<MessageId, Stream>;
  /** Tool calls announced for a message not stored yet (streaming or pending). */
  pendingActions: Record<MessageId, ToolAction[]>;
  /** Channels where the user has spoken and nothing has come back yet. */
  awaiting: Record<ChannelId, true>;
  /** Per channel, who is generating right now (from the engine's activity events). */
  typing: Record<ChannelId, Record<string, Typing>>;
  /** Per channel, the last turn that failed; cleared by the next send or activity there. */
  failures: Record<ChannelId, TurnFailure>;
  /** Messages that arrived in a channel while another was shown, until it is opened. */
  unread: Record<ChannelId, number>;
  /** Every person name seen in any payload, for labelling authors. */
  names: Record<PersonId, string>;
  /** Messages whose sources are unfolded; all start folded, and it lasts the session. */
  sourcesOpen: Record<MessageId, true>;
  /** Text a take-back put back for a channel's composer to pick up. */
  restore: { channel: ChannelId; text: string } | null;
  /** Bumped on each `WorldChanged`: screens holding fetched facts or tasks refetch. */
  worldVersion: number;
  /** Bumped on roster or mode changes: person pages refetch. */
  peopleVersion: number;
  loading: Partial<Record<LoadKey, true>>;
  loadErrors: Partial<Record<LoadKey, Failure>>;
  /** The channel in the main pane; `null` = home (host channel, or the library). */
  channel: ChannelId | null;
  drawer: Drawer | null;
  ask: AskDraft | null;
  groupDialog: boolean;
  libraryNoticeDismissed: boolean;
  /** Last failed action (not a load), announced in a live region. */
  error: Failure | null;
}

export const emptyWording: Wording = { entries: {}, scene_marker: { open: "*", close: "*" } };

export function initialState(): State {
  return {
    status: null,
    wording: emptyWording,
    t: makeT(emptyWording),
    clock: null,
    settings: null,
    endpoints: null,
    digest: { items: [] },
    coldstart: [],
    roster: null,
    quota: null,
    hostChannel: null,
    channels: [],
    messages: {},
    historyDone: {},
    streams: {},
    pendingActions: {},
    awaiting: {},
    typing: {},
    failures: {},
    unread: {},
    names: {},
    sourcesOpen: {},
    restore: null,
    worldVersion: 0,
    peopleVersion: 0,
    loading: {},
    loadErrors: {},
    channel: null,
    drawer: null,
    ask: null,
    groupDialog: false,
    libraryNoticeDismissed: false,
    error: null,
  };
}

export type Action =
  | { type: "status"; status: AppStatus }
  | { type: "wording"; wording: Wording }
  | { type: "clock"; clock: Clock }
  | { type: "settings"; settings: Settings }
  | { type: "endpoints"; endpoints: EndpointsView }
  | { type: "digest"; digest: Digest }
  | { type: "coldstart"; candidates: Candidate[] }
  | { type: "roster"; roster: Roster }
  | { type: "quota"; quota: QuotaStatus }
  | { type: "hostChannel"; id: ChannelId }
  | { type: "channels"; channels: ChannelSummary[] }
  /** A page of history: `older` pages go in front of what is loaded. */
  | { type: "history"; channel: ChannelId; messages: Message[]; older: boolean; done: boolean }
  | { type: "names"; persons: PersonRef[] }
  | { type: "messageDelta"; event: MessageDelta }
  | { type: "messageAdded"; event: MessageAdded }
  | { type: "toolCalled"; event: ToolCalled }
  | { type: "turnActivity"; event: TurnActivity }
  | { type: "modeChanged"; event: ModeChanged }
  | { type: "worldChanged"; event: WorldChanged }
  | { type: "rosterChanged" }
  | { type: "rewound"; event: Rewound }
  | { type: "restore"; restore: { channel: ChannelId; text: string } | null }
  | { type: "sources"; message: MessageId }
  | { type: "awaiting"; channel: ChannelId; on: boolean }
  | { type: "loading"; key: LoadKey }
  | { type: "loaded"; key: LoadKey; error: Failure | null }
  | { type: "open"; channel: ChannelId | null }
  | { type: "drawer"; drawer: Drawer | null }
  | { type: "ask"; draft: AskDraft | null }
  | { type: "groupDialog"; open: boolean }
  | { type: "dismissLibraryNotice" }
  | { type: "error"; error: Failure | null };
