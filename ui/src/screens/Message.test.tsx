import { act, screen } from "@testing-library/react";
import type { Message } from "../api";
import { renderWith } from "../test/render";
import { ChannelView } from "./Channel";
import { MessageView, StreamView } from "./Message";

const base: Message = {
  id: "m1",
  channel: "dm:mira",
  author: { kind: "person", id: "mira" },
  text: "",
  at: new Date().toISOString(),
  actions: [],
  cites: [],
  relayed: false,
  confidence: null,
  task: null,
  note: null,
  attachment: null,
};

test("citation marks, single or listed, never show in the speech", () => {
  const text = "The corridor closed in the first month.[#c-ledger-1,#c-ward-1] Oren disagrees [#c-ward-1].";
  const { container } = renderWith(<MessageView message={{ ...base, text }} showPlay={false} />);
  const speech = container.querySelector(".speech")!.textContent!;
  expect(speech).not.toMatch(/\[#|c-ledger|c-ward/);
  expect(speech).toContain("The corridor closed in the first month. Oren disagrees.");
});

test("angle-bracket markup in a reply is inert text, never an element", () => {
  const text = "<Mira mood=calm>Hello there.</Mira>";
  const { container } = renderWith(<MessageView message={{ ...base, text }} showPlay={false} />);
  expect(container.querySelector("mira")).toBeNull();
  expect(screen.getByText(/Hello there\./)).toBeDefined();
});

test("a three-paragraph reply is three bubbles with one author label and one sources list", () => {
  const text = "First thought.\n\nSecond thought.\n\nThird thought.\n\n[#c-ledger-1, #c-ward-1]";
  const cites: Message["cites"] = [
    { page: "Ledger", revid: 1, span_from: 0, span_to: 5, chunk_id: "c-ledger-1" },
    { page: "Ward", revid: 1, span_from: 0, span_to: 5, chunk_id: "c-ward-1" },
  ];
  const { container } = renderWith(<MessageView message={{ ...base, text, cites }} showPlay={false} />);
  const bubbles = [...container.querySelectorAll(".bubble")];
  expect(bubbles.map((b) => b.querySelector(".speech")!.textContent!.trim())).toEqual([
    "First thought.",
    "Second thought.",
    "Third thought.",
  ]);
  expect(container.querySelectorAll(".bubble .author")).toHaveLength(1);
  expect(container.querySelectorAll(".message-foot")).toHaveLength(1);
});

test("a trailing marks-only line makes no bubble", () => {
  const text = "One paragraph only.\n[#c-ledger-1]";
  const { container } = renderWith(<MessageView message={{ ...base, text }} showPlay={false} />);
  expect(container.querySelectorAll(".bubble")).toHaveLength(1);
  expect(container.querySelector(".speech")!.textContent!.trim()).toBe("One paragraph only.");
});

test("a streamed reply splits into bubbles as paragraphs arrive and hides a half-typed mark", () => {
  const { container } = renderWith(<StreamView author={base.author} text={"Hello.\n\nSecond one.\n\n[#c-led"} />);
  expect(container.querySelectorAll(".bubble")).toHaveLength(2);
});

test("the channel shows who is typing, a lookup wording for tools, and clears on done", async () => {
  const { store } = renderWith(<ChannelView channel="dm:mira" />);
  const author = { kind: "person", id: "mira" } as const;
  await act(async () => {
    store.dispatch({ type: "names", persons: [{ id: "mira", name: "Mira" }] });
    store.dispatch({ type: "turnActivity", event: { channel: "dm:mira", author, state: { kind: "thinking" } } });
  });
  // Like a chat client: in the header's subtitle, not as a line in the log.
  expect(screen.getByText("Mira is typing…").closest("header")).not.toBeNull();
  await act(async () => {
    store.dispatch({ type: "turnActivity", event: { channel: "dm:mira", author, state: { kind: "tool", name: "search" } } });
  });
  expect(screen.getByText("Mira is looking things up…")).toBeDefined();
  await act(async () => {
    store.dispatch({ type: "turnActivity", event: { channel: "dm:mira", author, state: { kind: "done" } } });
  });
  expect(screen.queryByText(/Mira is/)).toBeNull();
});

test("a host message that found people or made a group offers them, and a click switches the open channel", async () => {
  const { store } = renderWith(
    <MessageView
      showPlay={false}
      message={{
        ...base,
        channel: "host",
        author: { kind: "host" },
        text: "Here you are.",
        actions: [
          { tool: "find_people", topic: "ledgers", persons: [{ id: "mira", name: "Mira" }] },
          { tool: "create_group", members: [{ id: "mira", name: "Mira" }, { id: "oren", name: "Oren" }], channel: "grp:1" },
        ],
      }}
    />,
  );
  expect(screen.getByRole("button", { name: "Open the group with Mira, Oren" })).toBeDefined();
  await act(async () => {
    screen.getByRole("button", { name: "Open the group with Mira, Oren" }).click();
  });
  expect(store.getState().channel).toBe("grp:1");
  await act(async () => {
    screen.getByRole("button", { name: "Talk to Mira" }).click();
  });
  expect(store.getState().channel).toBe("dm:mira");
});

test("a host message that listed conversations offers each one", async () => {
  const { store } = renderWith(
    <MessageView
      showPlay={false}
      message={{
        ...base,
        channel: "host",
        author: { kind: "host" },
        text: "These are open.",
        actions: [
          {
            tool: "list_channels",
            channels: [
              { id: "grp:2", kind: "group", members: [{ id: "mira", name: "Mira" }, { id: "oren", name: "Oren" }] },
              { id: "dm:mira", kind: "direct", members: [{ id: "mira", name: "Mira" }] },
            ],
          },
        ],
      }}
    />,
  );
  await act(async () => {
    screen.getByRole("button", { name: "Open the group with Mira, Oren" }).click();
  });
  expect(store.getState().channel).toBe("grp:2");
  await act(async () => {
    screen.getByRole("button", { name: "Talk to Mira" }).click();
  });
  expect(store.getState().channel).toBe("dm:mira");
});

test("older rows without a group id or found people offer nothing", () => {
  const { container } = renderWith(
    <MessageView
      showPlay={false}
      message={{
        ...base,
        author: { kind: "host" },
        actions: [{ tool: "find_people", topic: "x" }, { tool: "create_group", members: [] }],
      }}
    />,
  );
  expect(container.querySelector(".choices")).toBeNull();
});

test("a failed turn is shown inline, with its reason, until the next send", async () => {
  const { store } = renderWith(<ChannelView channel="dm:mira" />);
  const author = { kind: "person", id: "mira" } as const;
  await act(async () => {
    store.dispatch({ type: "names", persons: [{ id: "mira", name: "Mira" }] });
    store.dispatch({ type: "turnActivity", event: { channel: "dm:mira", author, state: { kind: "failed", reason: "rate_limited" } } });
  });
  expect(screen.getByRole("alert").textContent).toMatch(/Mira could not reply: the endpoint is rate limiting/);
  await act(async () => {
    store.dispatch({ type: "awaiting", channel: "dm:mira", on: true });
  });
  expect(screen.queryByRole("alert")).toBeNull();
});

test("consecutive messages by one speaker form a run: one name, one avatar slot filled, at the end", () => {
  const first = { ...base, id: "a" };
  const last = { ...base, id: "b" };
  const { container } = renderWith(
    <>
      <MessageView message={first} showPlay={false} first last={false} />
      <MessageView message={last} showPlay={false} first={false} last />
    </>,
  );
  expect(container.querySelectorAll(".bubble .author")).toHaveLength(1);
  const slots = [...container.querySelectorAll(".message-avatar")];
  expect(slots.map((s) => s.children.length)).toEqual([0, 1]);
});
