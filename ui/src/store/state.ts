import type {
  ApiError,
  AppStatus,
  Author,
  Candidate,
  ChannelId,
  ChannelSummary,
  Clock,
  Digest,
  EndpointsView,
  Message,
  MessageAdded,
  MessageDelta,
  MessageId,
  ModeChanged,
  PersonId,
  PersonRef,
  QuotaStatus,
  Roster,
  Settings,
  TaskId,
  ToolAction,
  ToolCalled,
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
  | { kind: "system"; tab: "quota" | "endpoints" };

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
  /** Every person name seen in any payload, for labelling authors. */
  names: Record<PersonId, string>;
  /** Bumped on each `WorldChanged`: screens holding fetched facts or tasks refetch. */
  worldVersion: number;
  /** Bumped on roster or mode changes: person pages refetch. */
  peopleVersion: number;
  /** The channel in the main pane; `null` = home (host channel, or the library). */
  channel: ChannelId | null;
  drawer: Drawer | null;
  ask: AskDraft | null;
  groupDialog: boolean;
  libraryNoticeDismissed: boolean;
  /** Last error to announce; shown in a live region. */
  error: ApiError | null;
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
    names: {},
    worldVersion: 0,
    peopleVersion: 0,
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
  | { type: "modeChanged"; event: ModeChanged }
  | { type: "worldChanged"; event: WorldChanged }
  | { type: "rosterChanged" }
  | { type: "open"; channel: ChannelId | null }
  | { type: "drawer"; drawer: Drawer | null }
  | { type: "ask"; draft: AskDraft | null }
  | { type: "groupDialog"; open: boolean }
  | { type: "dismissLibraryNotice" }
  | { type: "error"; error: ApiError | null };
