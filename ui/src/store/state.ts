import type {
  ActivityFailure,
  AppInfo,
  AppStatus,
  Author,
  Candidate,
  ChannelId,
  ChannelSummary,
  Clock,
  Digest,
  EndpointsView,
  Failure,
  MediaFile,
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
  InterfaceLanguage,
  ToolAction,
  ToolCalled,
  TurnActivity,
  UpdateStatus,
  WorldChanged,
  Wording,
} from "../api";
import { makeT, type T } from "../i18n";
import { NO_FILTER, type RosterFilter } from "../lib/roster";

export type Drawer =
  | { kind: "roster" }
  /** The contacts list, a card over the app. */
  | { kind: "contacts" }
  | { kind: "person"; id: PersonId }
  /** The host's page, in the side panel of the host's channel. */
  | { kind: "host" }
  | { kind: "cases" }
  | { kind: "case"; id: TaskId }
  | { kind: "world" }
  | { kind: "system"; tab: SystemPage }
  /** A group's info panel (a person's is `person`); `rename` opens it with the name field ready. */
  | { kind: "channel"; id: ChannelId; rename?: boolean };

export type ProfileMode = "view" | "edit";

/** The pages of the settings card; `root` is its list. */
export type SystemPage = "root" | "language" | "quota" | "quiet" | "endpoints" | "saves" | "privacy" | "storage" | "about";

export type Theme = "system" | "light" | "dark";

/** Which list the chat column shows: the live chats, or the archived ones. */
export type ListView = "chats" | "archived";

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
  /** The words the interface speaks; the home's choice, shared with the terminal. */
  language: InterfaceLanguage;
  t: T;
  clock: Clock | null;
  /** The build (version, author) and the corpus's credit; `null` until read. */
  about: AppInfo | null;
  /** What the last update check found; `null` until read. */
  update: UpdateStatus | null;
  settings: Settings | null;
  endpoints: EndpointsView | null;
  digest: Digest;
  coldstart: Candidate[];
  roster: Roster | null;
  /** The roster's quick filters, kept while the window lives. */
  rosterFilter: RosterFilter;
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
  /** The user's picture (uploaded, else the default); `null` while unknown. */
  userAvatar: MediaFile | null;
  /** A conversation opened before it has any message: shown in the list while it is open. */
  temp: ChannelSummary | null;
  /** Chats kept at the top of the list; local to this window. */
  pinned: ChannelId[];
  theme: Theme;
  listView: ListView;
  /** The list was switched to the archived view by opening an archived conversation, so leaving it switches back. */
  listAuto: boolean;
  /** The left drawer (profile, settings shortcuts). */
  menuOpen: boolean;
  /** The user's own profile card: reading it, or editing it; `null` = closed. */
  profile: ProfileMode | null;
  /** Search inside the open conversation: `null` = off, else the text typed so far. */
  find: string | null;
  /** Every person name seen in any payload, for labelling authors. */
  names: Record<PersonId, string>;
  /** Messages whose sources are unfolded; all start folded, and it lasts the session. */
  sourcesOpen: Record<MessageId, true>;
  /** Text a take-back put back for a channel's composer to pick up. */
  restore: { channel: ChannelId; text: string } | null;
  /** Unsent text per channel, kept while the user is elsewhere; cleared on send. */
  drafts: Record<ChannelId, string>;
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
  /** A person's page shown as a card over the conversation (from a name or avatar in a chat). */
  personCard: PersonId | null;
  /** The host's page as a card, from its avatar or name in a chat. */
  hostCard: boolean;
  libraryNoticeDismissed: boolean;
  /** The library is the main pane on purpose (the user chose it); without a model it is always. */
  library: boolean;
  /** Last failed action (not a load), announced in a live region. */
  error: Failure | null;
}

export const emptyWording: Wording = { entries: {}, scene_marker: { open: "*", close: "*" } };

export function initialState(): State {
  return {
    status: null,
    wording: emptyWording,
    language: "corpus",
    t: makeT(emptyWording),
    clock: null,
    about: null,
    update: null,
    settings: null,
    endpoints: null,
    digest: { items: [] },
    coldstart: [],
    roster: null,
    rosterFilter: NO_FILTER,
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
    userAvatar: null,
    temp: null,
    pinned: [],
    theme: "system",
    listView: "chats",
    listAuto: false,
    menuOpen: false,
    profile: null,
    find: null,
    names: {},
    sourcesOpen: {},
    restore: null,
    drafts: {},
    worldVersion: 0,
    peopleVersion: 0,
    loading: {},
    loadErrors: {},
    channel: null,
    drawer: null,
    ask: null,
    groupDialog: false,
    personCard: null,
    hostCard: false,
    libraryNoticeDismissed: false,
    library: false,
    error: null,
  };
}

export type Action =
  /** Forget everything loaded from the world that was open (a log out); the window's own choices stay. */
  | { type: "reset" }
  | { type: "status"; status: AppStatus }
  | { type: "wording"; wording: Wording }
  | { type: "language"; language: InterfaceLanguage }
  | { type: "clock"; clock: Clock }
  | { type: "about"; about: AppInfo }
  | { type: "update"; update: UpdateStatus }
  | { type: "settings"; settings: Settings }
  | { type: "endpoints"; endpoints: EndpointsView }
  | { type: "digest"; digest: Digest }
  | { type: "coldstart"; candidates: Candidate[] }
  | { type: "roster"; roster: Roster }
  | { type: "rosterFilter"; filter: RosterFilter }
  | { type: "quota"; quota: QuotaStatus }
  | { type: "hostChannel"; id: ChannelId }
  | { type: "channels"; channels: ChannelSummary[] }
  /** A page of history: `older` pages go in front of what is loaded. */
  | { type: "history"; channel: ChannelId; messages: Message[]; older: boolean; done: boolean; merge?: boolean }
  | { type: "userAvatar"; avatar: MediaFile | null }
  | { type: "temp"; row: ChannelSummary | null }
  | { type: "prefs"; pinned: ChannelId[]; theme: Theme }
  | { type: "pin"; channel: ChannelId; on: boolean }
  | { type: "theme"; theme: Theme }
  | { type: "listView"; view: ListView }
  | { type: "menu"; open: boolean }
  | { type: "profile"; mode: ProfileMode | null }
  | { type: "find"; query: string | null }
  /** Set a channel's unread count by hand: 0 = read, 1 = marked unread. */
  | { type: "unread"; channel: ChannelId; count: number }
  | { type: "names"; persons: PersonRef[] }
  | { type: "messageDelta"; event: MessageDelta }
  | { type: "messageAdded"; event: MessageAdded }
  | { type: "toolCalled"; event: ToolCalled }
  /** A suggested group was made: its action line now names the group, so it reads as opened. */
  | { type: "offerTaken"; channel: ChannelId; message: MessageId; action: number; group: ChannelId }
  | { type: "turnActivity"; event: TurnActivity }
  | { type: "modeChanged"; event: ModeChanged }
  | { type: "worldChanged"; event: WorldChanged }
  | { type: "rosterChanged" }
  | { type: "rewound"; event: Rewound }
  | { type: "restore"; restore: { channel: ChannelId; text: string } | null }
  | { type: "draft"; channel: ChannelId; text: string }
  | { type: "sources"; message: MessageId }
  | { type: "awaiting"; channel: ChannelId; on: boolean }
  | { type: "loading"; key: LoadKey }
  | { type: "loaded"; key: LoadKey; error: Failure | null }
  | { type: "open"; channel: ChannelId | null }
  | { type: "drawer"; drawer: Drawer | null }
  | { type: "ask"; draft: AskDraft | null }
  | { type: "personCard"; person: PersonId | null }
  | { type: "hostCard"; open: boolean }
  | { type: "groupDialog"; open: boolean }
  | { type: "dismissLibraryNotice" }
  | { type: "library"; on: boolean }
  | { type: "error"; error: Failure | null };
