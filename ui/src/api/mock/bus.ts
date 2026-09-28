import type { EventSource, Events } from "..";

/** In-memory event channels with the same `listen` shape as the generated events. */
export function createBus() {
  const make = <T>() => {
    const listeners = new Set<(e: { payload: T }) => void>();
    const source: EventSource<T> & { emit(payload: T): void } = {
      listen(cb) {
        listeners.add(cb);
        return Promise.resolve(() => void listeners.delete(cb));
      },
      emit(payload) {
        for (const l of [...listeners]) l({ payload });
      },
    };
    return source;
  };
  return {
    messageAdded: make<import("../../api.gen").MessageAdded>(),
    messageDelta: make<import("../../api.gen").MessageDelta>(),
    modeChanged: make<import("../../api.gen").ModeChanged>(),
    notification: make<import("../../api.gen").Notification>(),
    quotaChanged: make<import("../../api.gen").QuotaChanged>(),
    rosterChanged: make<import("../../api.gen").RosterChanged>(),
    toolCalled: make<import("../../api.gen").ToolCalled>(),
    worldChanged: make<import("../../api.gen").WorldChanged>(),
  } satisfies Events;
}

export type Bus = ReturnType<typeof createBus>;
