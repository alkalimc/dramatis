// The one seam between the screens and the engine. Inside the Tauri webview these are the
// generated bindings; anywhere else (browser preview, tests) an in-memory mock with the
// same signatures. Screens import from here only, never from `api.gen` directly.
import { convertFileSrc, isTauri } from "@tauri-apps/api/core";
import * as gen from "../api.gen";
import { createMock, type MockOptions } from "./mock";

export type * from "../api.gen";

export type Commands = typeof gen.commands;

/** What every generated command returns. */
export type Result<T> = Promise<{ status: "ok"; data: T } | { status: "error"; error: gen.ApiError }>;

/** The part of a generated event the UI uses; the mock implements just this. */
export interface EventSource<T> {
  listen(cb: (event: { payload: T }) => void): Promise<() => void>;
}

/** The payload a generated event delivers to its listener. */
type PayloadOf<E> = E extends { listen(cb: (event: { payload: infer P }) => void): unknown } ? P : never;

/** One entry per generated event, so a new event fails to compile until it is handled. */
export type Events = { [K in keyof typeof gen.events]: EventSource<PayloadOf<(typeof gen.events)[K]>> };

export interface Api {
  commands: Commands;
  events: Events;
  /** Turn a `MediaFile.path` into something an `<img>` or `<audio>` can load. */
  mediaUrl(path: string): string;
  mock: boolean;
}

/** The generated bindings, as the webview sees them. */
export function tauriApi(): Api {
  return { commands: gen.commands, events: gen.events, mediaUrl: (p) => (p.startsWith("data:") ? p : convertFileSrc(p)), mock: false };
}

export function mockApi(params: URLSearchParams, opts?: MockOptions): Api {
  const mock = createMock(params, opts);
  return { ...mock, mediaUrl: (p) => p, mock: true };
}

function inTauri(): boolean {
  return typeof window !== "undefined" && ("__TAURI_INTERNALS__" in window || isTauri());
}

export const api: Api = inTauri() ? tauriApi() : mockApi(new URLSearchParams(typeof location === "undefined" ? "" : location.search));

/**
 * A failed call: the engine's `ApiError`, or anything else the IPC layer threw (a command
 * the engine does not register, a broken bridge), kept as text.
 */
export type Failure = gen.ApiError | { code: "unknown"; detail: string };

// Every engine error code. A record, so a code added on the engine side fails to compile
// here until the UI has a key for it.
const CODES: Record<gen.ApiError["code"], true> = {
  not_implemented: true,
  not_found: true,
  invalid: true,
  disabled: true,
  quota_exhausted: true,
  no_endpoint: true,
  no_persona: true,
  endpoint: true,
  keychain: true,
  io: true,
  busy: true,
  exists: true,
  locked: true,
  wrong_password: true,
};

export function asFailure(e: unknown): Failure {
  if (typeof e === "object" && e !== null && "code" in e && typeof e.code === "string" && e.code in CODES) {
    return e as gen.ApiError;
  }
  const detail = e instanceof Error ? e.message : typeof e === "string" ? e : JSON.stringify(e);
  return { code: "unknown", detail: detail ?? "" };
}

/** Await a command without ever throwing: its data, or why it failed. */
export async function settle<T>(p: Result<T>): Promise<{ ok: true; data: T } | { ok: false; error: Failure }> {
  try {
    const r = await p;
    return r.status === "ok" ? { ok: true, data: r.data } : { ok: false, error: asFailure(r.error) };
  } catch (e) {
    return { ok: false, error: asFailure(e) };
  }
}
