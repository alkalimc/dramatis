import { act, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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
  picture_lost: false,
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

test("a person's group suggestion makes the group only when pressed, and only once", async () => {
  // The engine records the group on the stored line; the stand-in does the same.
  let made: string | null = null;
  let calls = 0;
  const { store } = renderWith(<ChannelView channel="dm:mira" />, {
    patch: (api) => ({
      ...api,
      commands: {
        ...api.commands,
        openOffer: async () => {
          calls++;
          if (made) return { status: "ok", data: made };
          const r = await api.commands.createGroup(["mira", "oren"], null);
          if (r.status === "ok") made = r.data;
          return r;
        },
      },
    }),
  });
  const suggestion: Message = {
    ...base,
    channel: "dm:mira",
    text: "Oren knows this better than I do.",
    actions: [{ tool: "create_group", members: [{ id: "mira", name: "Mira" }, { id: "oren", name: "Oren" }], channel: null }],
  };
  await act(async () => {
    store.dispatch({ type: "names", persons: [{ id: "mira", name: "Mira" }] });
    store.dispatch({ type: "messageAdded", event: { message: suggestion } });
  });
  expect(screen.getByText(/suggesting a group with Mira, Oren/)).toBeDefined();
  expect(store.getState().channel).toBeNull();
  await act(async () => {
    screen.getByRole("button", { name: "Start a group with Mira, Oren" }).click();
  });
  const open = store.getState().channel;
  expect(store.getState().channels.find((c) => c.id === open)?.kind).toBe("group");
  // The button now reads as opened and goes to the same group.
  expect(screen.queryByRole("button", { name: "Start a group with Mira, Oren" })).toBeNull();
  await act(async () => {
    store.dispatch({ type: "open", channel: "dm:mira" });
  });
  await act(async () => {
    screen.getByRole("button", { name: "Open the group with Mira, Oren" }).click();
  });
  expect(store.getState().channel).toBe(open);
  expect(calls).toBe(1);
  expect(made).toBe(open);
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

test("the user's own messages sit on the right with their picture, others on the left", () => {
  const mine = renderWith(<MessageView message={{ ...base, author: { kind: "user" }, text: "Hello." }} showPlay={false} />).container.querySelector("article")!;
  expect(mine.className).toContain("from-user");
  // The picture slot comes after the bubbles; for others it comes before.
  expect(mine.lastElementChild!.className).toBe("message-avatar");
  expect(mine.firstElementChild!.className).toBe("message-stack");
  const theirs = renderWith(<MessageView message={{ ...base, text: "Hello." }} showPlay={false} />).container.querySelector("article")!;
  expect(theirs.className).toContain("from-other");
  expect(theirs.firstElementChild!.className).toBe("message-avatar");
});

test("right click on a message offers to copy it, and to send an own message again", async () => {
  const user = userEvent.setup();
  const { container } = renderWith(<MessageView message={{ ...base, author: { kind: "user" }, text: "Hello there" }} showPlay={false} />);
  await user.pointer({ keys: "[MouseRight]", target: container.querySelector(".bubble")! });
  expect(await screen.findByRole("menuitem", { name: "Copy text" })).toBeDefined();
  expect(screen.getByRole("menuitem", { name: "Send again" })).toBeDefined();
});

const mine: Message = { ...base, author: { kind: "user" }, text: "look at this" };
const url = "data:image/png;base64,iVBORw0KGgo=";

test("a sent picture shows in the user's bubble and opens larger on a click", async () => {
  const user = userEvent.setup();
  const { container } = renderWith(<MessageView message={{ ...mine, attachment: { path: url } }} showPlay={false} />);
  const img = container.querySelector(".from-user .bubble img.attachment") as HTMLImageElement;
  expect(img.getAttribute("src")).toBe(url);
  await user.click(screen.getByRole("button", { name: "Open the picture larger" }));
  const dialog = await screen.findByRole("dialog");
  expect(dialog.querySelector("img")!.getAttribute("src")).toBe(url);
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("a picture sent without words is still one bubble holding the picture", () => {
  const { container } = renderWith(<MessageView message={{ ...mine, text: "", attachment: { path: url } }} showPlay={false} />);
  expect(container.querySelectorAll(".bubble")).toHaveLength(1);
  expect(container.querySelector(".bubble img.attachment")).not.toBeNull();
});

test("a picture that is gone shows a placeholder instead of a broken image", () => {
  const { container } = renderWith(<MessageView message={{ ...mine, picture_lost: true }} showPlay={false} />);
  expect(container.querySelector("img")).toBeNull();
  expect(screen.getByText("Picture unavailable")).toBeDefined();
  expect(screen.getByText("look at this")).toBeDefined();
});
