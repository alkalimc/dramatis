import type { Message } from "../api";

/** Delivery of one of the user's messages: 0 none, 1 stored and a reply under way, 2 the replies are in. */
export type Delivery = 0 | 1 | 2;

const spoken = (m: Message) => !m.service;

/**
 * `list` is the channel's messages, oldest first. `busy` is whether anyone there is typing or
 * streaming right now; `failed` whether the last turn there ended without a reply. A message
 * followed by someone else's message was answered, so history keeps its double ticks.
 */
export function deliveryOf(list: Message[], id: string, busy: boolean, failed: boolean): Delivery {
  const i = list.findIndex((m) => m.id === id);
  if (i < 0) return 0;
  const reply = list.findIndex((m, k) => k > i && spoken(m) && m.author.kind !== "user");
  const lastUser = list.findLastIndex((m) => spoken(m) && m.author.kind === "user");
  // A reply that came before a later message of the user's closed its round.
  if (reply >= 0 && reply < lastUser) return 2;
  // Otherwise this message is part of the round now open (or just finished).
  if (busy) return 1;
  if (reply < 0) return 0;
  return failed ? 1 : 2;
}
