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
    awayChanged: make<import("../gen").AwayChanged>(),
    messageAdded: make<import("../gen").MessageAdded>(),
    messageDelta: make<import("../gen").MessageDelta>(),
    modeChanged: make<import("../gen").ModeChanged>(),
    notification: make<import("../gen").Notification>(),
    quotaChanged: make<import("../gen").QuotaChanged>(),
    rewound: make<import("../gen").Rewound>(),
    rosterChanged: make<import("../gen").RosterChanged>(),
    toolCalled: make<import("../gen").ToolCalled>(),
    turnActivity: make<import("../gen").TurnActivity>(),
    worldChanged: make<import("../gen").WorldChanged>(),
  } satisfies Events;
}

export type Bus = ReturnType<typeof createBus>;
