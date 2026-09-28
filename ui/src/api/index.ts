// The one seam between the screens and the engine. Inside the Tauri webview these are the
// generated bindings; anywhere else (browser preview, tests) an in-memory mock with the
// same signatures. Screens import from here only, never from `api.gen` directly.
import { convertFileSrc, isTauri } from "@tauri-apps/api/core";
import * as gen from "../api.gen";
import { createMock } from "./mock";

export type * from "../api.gen";

export type Commands = typeof gen.commands;

/** The part of a generated event the UI uses; the mock implements just this. */
export interface EventSource<T> {
  listen(cb: (event: { payload: T }) => void): Promise<() => void>;
}

export interface Events {
  messageAdded: EventSource<gen.MessageAdded>;
  messageDelta: EventSource<gen.MessageDelta>;
  modeChanged: EventSource<gen.ModeChanged>;
  notification: EventSource<gen.Notification>;
  quotaChanged: EventSource<gen.QuotaChanged>;
  rosterChanged: EventSource<gen.RosterChanged>;
  toolCalled: EventSource<gen.ToolCalled>;
  worldChanged: EventSource<gen.WorldChanged>;
}

export interface Api {
  commands: Commands;
  events: Events;
  /** Turn a `MediaFile.path` into something an `<img>` or `<audio>` can load. */
  mediaUrl(path: string): string;
  mock: boolean;
}

function inTauri(): boolean {
  return typeof window !== "undefined" && ("__TAURI_INTERNALS__" in window || isTauri());
}

function pick(): Api {
  if (inTauri()) {
    return { commands: gen.commands, events: gen.events, mediaUrl: (p) => convertFileSrc(p), mock: false };
  }
  const params = typeof location === "undefined" ? "" : location.search;
  return { ...createMock(new URLSearchParams(params)), mediaUrl: (p) => p, mock: true };
}

export const api: Api = pick();

/** Unwrap a command result or throw its `ApiError` (screens catch and show it). */
export async function call<T>(p: Promise<{ status: "ok"; data: T } | { status: "error"; error: gen.ApiError }>): Promise<T> {
  const r = await p;
  if (r.status === "error") throw new ApiFailure(r.error);
  return r.data;
}

export class ApiFailure extends Error {
  constructor(readonly error: gen.ApiError) {
    super(error.code);
  }
}
