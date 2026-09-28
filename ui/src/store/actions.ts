import { ApiFailure, call, type Api, type ChannelId, type Mode, type Settings, type Target } from "../api";
import type { Store } from ".";

const PAGE = 50;

/**
 * Everything the screens do that touches the engine: command, then fold the result into
 * the store. Events keep the store current afterwards; nothing here polls.
 */
export function createActions(api: Api, store: Store) {
  const { commands: c } = api;
  const d = store.dispatch;

  /** Run a command; on failure announce it and resolve `undefined`. */
  async function attempt<T>(p: Promise<{ status: "ok"; data: T } | { status: "error"; error: import("../api").ApiError }>): Promise<T | undefined> {
    try {
      return await call(p);
    } catch (e) {
      if (e instanceof ApiFailure) {
        d({ type: "error", error: e.error });
        return undefined;
      }
      throw e;
    }
  }

  async function refreshDigest() {
    const digest = await attempt(c.digest());
    if (digest) d({ type: "digest", digest });
  }
  async function refreshRoster() {
    const roster = await attempt(c.roster());
    if (roster) d({ type: "roster", roster });
  }
  async function refreshChannels() {
    const channels = await attempt(c.channels());
    if (channels) d({ type: "channels", channels });
  }
  async function refreshQuota() {
    const quota = await attempt(c.quota());
    if (quota) d({ type: "quota", quota });
  }
  async function refreshEndpoints() {
    const endpoints = await attempt(c.endpoints());
    if (endpoints) d({ type: "endpoints", endpoints });
  }
  async function refreshClock() {
    // The clock is a pure function of the wall clock; a failure just leaves it blank.
    const r = await c.clock();
    if (r.status === "ok") d({ type: "clock", clock: r.data });
  }

  /** Status decides the form; the full form also needs the host channel and presence. */
  async function refreshStatus() {
    const status = await attempt(c.appStatus());
    if (!status) return;
    d({ type: "status", status });
    if (status.form === "full" && !status.needs_setup) await enterFull();
  }

  let presenceSent = false;
  async function enterFull() {
    const host = await attempt(c.hostChannel());
    if (!host) return;
    d({ type: "hostChannel", id: host });
    await loadHistory(host);
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
    const [wording, settings, coldstart] = await Promise.all([
      attempt(c.wording()),
      attempt(c.settings()),
      attempt(c.coldstart()),
    ]);
    if (wording) d({ type: "wording", wording });
    if (settings) d({ type: "settings", settings });
    if (coldstart) d({ type: "coldstart", candidates: coldstart });
    await Promise.all([refreshEndpoints(), refreshRoster(), refreshChannels(), refreshQuota(), refreshClock()]);
    await refreshStatus();
  }

  async function loadHistory(channel: ChannelId, older = false) {
    const loaded = store.getState().messages[channel] ?? [];
    if (!older && loaded.length > 0) return;
    const before = older && loaded.length > 0 ? loaded[0].id : null;
    const page = await attempt(c.history(channel, before, PAGE));
    if (!page) return;
    d({ type: "history", channel, messages: page, older, done: page.length < PAGE });
  }

  async function markRead(channel: ChannelId) {
    const list = store.getState().messages[channel];
    const last = list?.[list.length - 1];
    if (last) await attempt(c.markRead(channel, last.id));
  }

  async function open(channel: ChannelId | null) {
    d({ type: "open", channel });
    const target = channel ?? store.getState().hostChannel;
    if (!target) return;
    await loadHistory(target);
    await markRead(target);
  }

  async function openDirect(person: string) {
    const channel = await attempt(c.openDirect(person));
    if (!channel) return;
    if (!store.getState().channels.some((ch) => ch.id === channel)) await refreshChannels();
    await open(channel);
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

  return {
    attempt,
    boot,
    presence,
    refreshStatus,
    refreshDigest,
    refreshRoster,
    refreshChannels,
    refreshQuota,
    refreshEndpoints,
    refreshClock,
    loadHistory,
    markRead,
    open,
    openDirect,
    setMode,
    saveSettings,
  };
}

export type Actions = ReturnType<typeof createActions>;

/** Feed engine events into the store. Returns the unsubscribe. */
export async function subscribe(api: Api, store: Store, actions: Actions): Promise<() => void> {
  const e = api.events;
  const d = store.dispatch;
  const offs = await Promise.all([
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
    e.toolCalled.listen(({ payload }) => d({ type: "toolCalled", event: payload })),
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
      if (kinds.has("channel")) void actions.refreshChannels();
      if (kinds.has("trust")) {
        d({ type: "rosterChanged" });
        void actions.refreshRoster();
      }
    }),
  ]);
  return () => offs.forEach((off) => off());
}
