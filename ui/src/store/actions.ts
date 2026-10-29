import { asFailure, settle, type Api, type ChannelId, type ChannelSummary, type ImagePart, type InterfaceLanguage, type Mode, type PersonRef, type Result, type Settings, type Target } from "../api";
import { loadPrefs, savePrefs } from "../lib/prefs";
import type { Store } from ".";
import type { LoadKey, Theme } from "./state";

const PAGE = 50;

/**
 * Everything the screens do that touches the engine: command, then fold the result into
 * the store. Events keep the store current afterwards; nothing here polls.
 */
export function createActions(api: Api, store: Store) {
  const { commands: c } = api;
  const d = store.dispatch;

  /**
   * Run a command without ever throwing. With a `key` the outcome lands in the store's
   * load state (the screen shows it inline, with a retry); without one a failure is
   * announced in the error bar. Resolves `undefined` on failure.
   */
  async function attempt<T>(p: Result<T>, key?: LoadKey): Promise<T | undefined> {
    if (key) d({ type: "loading", key });
    const r = await settle(p);
    if (key) d({ type: "loaded", key, error: r.ok ? null : r.error });
    else if (!r.ok) d({ type: "error", error: r.error });
    else if (store.getState().error) d({ type: "error", error: null });
    return r.ok ? r.data : undefined;
  }

  async function refreshDigest() {
    const digest = await attempt(c.digest(), "digest");
    if (digest) d({ type: "digest", digest });
  }
  async function refreshRoster() {
    const roster = await attempt(c.roster(), "roster");
    if (roster) d({ type: "roster", roster });
  }
  async function refreshChannels() {
    const channels = await attempt(c.channels(), "channels");
    if (channels) d({ type: "channels", channels });
  }
  async function refreshUserAvatar() {
    const r = await settle(c.userAvatar());
    d({ type: "userAvatar", avatar: r.ok ? r.data : null });
  }
  async function refreshQuota() {
    const quota = await attempt(c.quota(), "quota");
    if (quota) d({ type: "quota", quota });
  }
  async function refreshEndpoints() {
    const endpoints = await attempt(c.endpoints(), "endpoints");
    if (endpoints) d({ type: "endpoints", endpoints });
  }
  async function refreshClock() {
    // The clock is a pure function of the wall clock; a failure just leaves it blank.
    const r = await c.clock();
    if (r.status === "ok") d({ type: "clock", clock: r.data });
  }

  /** Status decides the form; the full form also needs the host channel and presence. */
  async function refreshStatus() {
    const status = await attempt(c.appStatus(), "status");
    if (!status) return;
    d({ type: "status", status });
    if (status.form === "full" && !status.needs_setup) await enterFull();
  }

  let presenceSent = false;
  async function enterFull() {
    const host = await attempt(c.hostChannel(), "host");
    if (!host) return;
    d({ type: "hostChannel", id: host });
    await loadHistory(host);
    await markShownRead();
    if (!presenceSent) {
      presenceSent = true;
      await presence();
    }
  }

  /** Opened, or back from being away: the waiting panel at once, and whatever is said. */
  async function presence() {
    const digest = await attempt(c.presence());
    if (digest) d({ type: "digest", digest });
  }

  async function boot() {
    const [wording, language, settings, coldstart] = await Promise.all([
      attempt(c.wording(), "wording"),
      settle(c.interfaceLanguage()),
      attempt(c.settings(), "settings"),
      attempt(c.coldstart(), "coldstart"),
    ]);
    if (wording) d({ type: "wording", wording });
    if (language.ok) d({ type: "language", language: language.data });
    if (settings) d({ type: "settings", settings });
    if (coldstart) d({ type: "coldstart", candidates: coldstart });
    const prefs = loadPrefs();
    d({ type: "prefs", pinned: prefs.pinned, theme: prefs.theme });
    await Promise.all([refreshEndpoints(), refreshRoster(), refreshChannels(), refreshQuota(), refreshClock(), refreshUserAvatar()]);
    await refreshStatus();
  }

  async function loadHistory(channel: ChannelId, older = false) {
    const loaded = store.getState().messages[channel] ?? [];
    if (!older && loaded.length > 0) return;
    const before = older && loaded.length > 0 ? loaded[0].id : null;
    const page = await attempt(c.history(channel, before, PAGE), `history:${channel}`);
    if (!page) return;
    d({ type: "history", channel, messages: page, older, done: page.length < PAGE });
  }

  /** Fetch the newest page again and fold it in: another process may have written. */
  async function reloadHistory(channel: ChannelId) {
    const page = await attempt(c.history(channel, null, PAGE));
    if (page) d({ type: "history", channel, messages: page, older: false, done: store.getState().historyDone[channel] ?? page.length < PAGE, merge: true });
  }

  async function markRead(channel: ChannelId) {
    const list = store.getState().messages[channel];
    const last = list?.[list.length - 1];
    if (last && (await attempt(c.markRead(channel, last.id))) !== undefined) d({ type: "unread", channel, count: 0 });
  }

  /** The conversation on screen has been seen: at start, and when the window comes back. */
  async function markShownRead() {
    const s = store.getState();
    const shown = s.channel ?? s.hostChannel;
    if (shown && !document.hidden) {
      await markRead(shown);
      await refreshDigest();
    }
  }

  /**
   * Open any conversation, from wherever. `who` is who it is with (one person, or the
   * members of a group), so one nobody has spoken in yet can show a temporary row. One that
   * is newer than the list (a group just made) is looked up first.
   */
  async function open(channel: ChannelId | null, who?: PersonRef | PersonRef[]) {
    if (channel && !store.getState().channels.some((ch) => ch.id === channel)) {
      await refreshChannels();
      if (who && !store.getState().channels.some((ch) => ch.id === channel)) {
        const group = Array.isArray(who);
        d({ type: "temp", row: { id: channel, kind: group ? "group" : "direct", origin: "system", participants: group ? who : [who], topic: null, mode: "enabled", last_at: null, name: null, avatar: null, archived: false, unread: 0 } satisfies ChannelSummary });
      }
    }
    d({ type: "open", channel });
    const target = channel ?? store.getState().hostChannel;
    if (!target) return;
    await loadHistory(target);
    await markRead(target);
  }

  function personRef(person: string, name?: string): PersonRef {
    return { id: person, name: name ?? store.getState().names[person] ?? person };
  }

  async function openDirect(person: string) {
    const channel = await attempt(c.openDirect(person));
    if (channel) await open(channel, personRef(person));
  }

  /** Take a person a find suggestion offered: their chat opens with the reason they were found. */
  async function openFound(message: string, action: number, person: string, name?: string) {
    const channel = await attempt(c.openFound(message, action, person));
    if (!channel) return;
    await open(channel, personRef(person, name));
  }

  /**
   * Take one of the user's messages back with everything after it, and hand its text to
   * the channel's composer. The engine refuses while a reply is being written there.
   */
  async function resend(channel: ChannelId, message: string) {
    const text = await attempt(c.rewind(channel, message));
    if (text === undefined) return;
    d({ type: "rewound", event: { channel, from: message } });
    d({ type: "restore", restore: { channel, text } });
  }

  /** Mark a channel read by hand (the badge, the context menu). */
  async function clearUnread(channel: ChannelId) {
    await loadHistory(channel);
    d({ type: "unread", channel, count: 0 });
    await markRead(channel);
    await refreshDigest();
  }

  function markUnread(channel: ChannelId) {
    d({ type: "unread", channel, count: 1 });
  }

  function pin(channel: ChannelId, on: boolean) {
    d({ type: "pin", channel, on });
    const s = store.getState();
    savePrefs({ pinned: s.pinned, theme: s.theme });
  }

  function setTheme(theme: Theme) {
    d({ type: "theme", theme });
    savePrefs({ pinned: store.getState().pinned, theme });
  }

  async function archive(channel: ChannelId, archived: boolean) {
    if ((await attempt(c.archiveChannel(channel, archived))) === undefined) return;
    await refreshChannels();
    if (archived && (store.getState().channel ?? store.getState().hostChannel) === channel) await open(null);
  }
  async function rename(channel: ChannelId, name: string | null) {
    if ((await attempt(c.renameChannel(channel, name))) !== undefined) await refreshChannels();
  }
  async function setChannelAvatar(channel: ChannelId, image: ImagePart | null) {
    if ((await attempt(c.setChannelAvatar(channel, image))) !== undefined) await refreshChannels();
  }
  async function setUserAvatar(image: ImagePart | null) {
    if ((await attempt(c.setUserAvatar(image))) !== undefined) await refreshUserAvatar();
  }

  async function setMode(target: Target, mode: Mode) {
    // The store follows the `ModeChanged` event, which also covers changes the host makes.
    await attempt(c.setMode(target, mode));
  }

  async function saveSettings(next: Settings) {
    const ok = await attempt(c.updateSettings(next));
    if (ok === undefined) return false;
    d({ type: "settings", settings: next });
    await refreshStatus();
    return true;
  }

  /** Switch the interface language: kept in the home, applied at once by reading the wording again. */
  async function setInterfaceLanguage(language: InterfaceLanguage) {
    if ((await attempt(c.setInterfaceLanguage(language))) === undefined) return false;
    d({ type: "language", language });
    const wording = await attempt(c.wording(), "wording");
    if (wording) d({ type: "wording", wording });
    return true;
  }

  /**
   * Close the open save and show the save list. What was loaded belonged to that world, so
   * it goes; the status that follows says no save is open.
   */
  async function logOut() {
    if ((await attempt(c.logOut())) === undefined) return false;
    d({ type: "reset" });
    await boot();
    return true;
  }

  return {
    attempt,
    boot,
    logOut,
    presence,
    refreshStatus,
    refreshDigest,
    refreshRoster,
    refreshChannels,
    refreshQuota,
    refreshEndpoints,
    refreshClock,
    loadHistory,
    reloadHistory,
    markRead,
    markShownRead,
    clearUnread,
    markUnread,
    pin,
    setTheme,
    archive,
    rename,
    setChannelAvatar,
    setUserAvatar,
    open,
    openDirect,
    openFound,
    resend,
    setMode,
    saveSettings,
    setInterfaceLanguage,
  };
}

export type Actions = ReturnType<typeof createActions>;

/** Feed engine events into the store. Returns the unsubscribe. */
export async function subscribe(api: Api, store: Store, actions: Actions): Promise<() => void> {
  const offs: (() => void)[] = [];
  try {
    offs.push(...(await listenAll(api, store, actions)));
    store.dispatch({ type: "loaded", key: "events", error: null });
  } catch (err) {
    store.dispatch({ type: "loaded", key: "events", error: asFailure(err) });
  }
  return () => offs.forEach((off) => off());
}

async function listenAll(api: Api, store: Store, actions: Actions): Promise<(() => void)[]> {
  const e = api.events;
  const d = store.dispatch;
  return Promise.all([
    e.messageDelta.listen(({ payload }) => d({ type: "messageDelta", event: payload })),
    e.messageAdded.listen(({ payload }) => {
      d({ type: "messageAdded", event: payload });
      const s = store.getState();
      const shown = s.channel ?? s.hostChannel;
      if (payload.message.channel === shown && !document.hidden) void actions.markRead(shown);
      if (!s.channels.some((ch) => ch.id === payload.message.channel) && payload.message.channel !== s.hostChannel) {
        void actions.refreshChannels();
      }
    }),
    // The OS notification is the engine's; here it only means a channel may be new.
    e.notification.listen(({ payload }) => {
      const s = store.getState();
      if (!s.channels.some((ch) => ch.id === payload.channel) && payload.channel !== s.hostChannel) void actions.refreshChannels();
    }),
    e.toolCalled.listen(({ payload }) => d({ type: "toolCalled", event: payload })),
    e.turnActivity.listen(({ payload }) => d({ type: "turnActivity", event: payload })),
    e.rewound.listen(({ payload }) => {
      d({ type: "rewound", event: payload });
      void actions.refreshChannels();
      void actions.refreshDigest();
    }),
    e.modeChanged.listen(({ payload }) => d({ type: "modeChanged", event: payload })),
    e.rosterChanged.listen(() => {
      d({ type: "rosterChanged" });
      void actions.refreshRoster();
    }),
    e.quotaChanged.listen(({ payload }) => d({ type: "quota", quota: payload.status })),
    e.worldChanged.listen(({ payload }) => {
      d({ type: "worldChanged", event: payload });
      const kinds = new Set(payload.changes.map((ch) => ch.kind));
      if (kinds.has("digest") || kinds.has("task")) void actions.refreshDigest();
      if (kinds.has("channel")) {
        // Another process (the terminal) may have written: the list and what is on screen.
        void actions.refreshChannels();
        const s = store.getState();
        const shown = s.channel ?? s.hostChannel;
        const touched = payload.changes.some((ch) => ch.kind === "channel" && ch.id === shown);
        if (shown && touched) void actions.reloadHistory(shown).then(() => (document.hidden ? undefined : actions.markRead(shown)));
      }
      if (kinds.has("trust")) {
        d({ type: "rosterChanged" });
        void actions.refreshRoster();
      }
    }),
  ]);
}
