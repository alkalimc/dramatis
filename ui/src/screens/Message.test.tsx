import { act, screen, within } from "@testing-library/react";
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
 
  confidence: null,
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
  // The stand-in has no stored line for this message; it takes the person like a plain chat.
  const taken: unknown[][] = [];
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
    {
      patch: (api) => ({
        ...api,
        commands: {
          ...api.commands,
          openFound: async (...args) => {
            taken.push(args);
            return api.commands.openDirect(args[2]);
          },
        },
      }),
    },
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
  // A found person is taken through the line they came from, so the reason is written.
  expect(taken).toEqual([["m1", 0, "mira"]]);
});

test("a message with no text and no picture draws no bubble; its offers stand alone under a plain line", () => {
  const empty = { ...base, channel: "host", author: { kind: "host" } as const, text: "  \n " };
  const plain = renderWith(<MessageView showPlay={false} message={empty} />);
  expect(plain.container.querySelector(".bubble")).toBeNull();
  expect(plain.container.querySelector("article")).toBeNull();
  expect(plain.container.querySelector(".service-line")).toBeNull();
  plain.unmount();

  const offers = renderWith(
    <MessageView showPlay={false} message={{ ...empty, actions: [{ tool: "find_people", topic: "ledgers", persons: [{ id: "mira", name: "Mira" }] }] }} />,
  );
  expect(offers.container.querySelector(".bubble")).toBeNull();
  expect(offers.container.querySelector(".service-line")?.textContent).toBe("Here are some suggestions.");
  expect(screen.getByRole("button", { name: "Talk to Mira" })).toBeDefined();
  offers.unmount();

  // A picture alone is still a bubble.
  const picture = renderWith(<MessageView showPlay={false} message={{ ...empty, picture_lost: true }} />);
  expect(picture.container.querySelectorAll(".bubble").length).toBe(1);
});

test("a service line is a quiet note, not a bubble", () => {
  const { container } = renderWith(<MessageView showPlay={false} message={{ ...base, author: { kind: "user" }, service: true, text: "Amia suggested you could help the Doctor with: ledgers" }} />);
  expect(container.querySelector(".service-line")?.textContent).toBe("Amia suggested you could help the Doctor with: ledgers");
  expect(container.querySelector(".bubble")).toBeNull();
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
  const first = { ...base, id: "a", text: "One." };
  const last = { ...base, id: "b", text: "Two." };
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

test("Send again has no button of its own, and selecting text opens nothing", async () => {
  const user = userEvent.setup();
  const { container } = renderWith(<MessageView message={{ ...base, author: { kind: "user" }, text: "Hello there" }} showPlay={false} />);
  expect(screen.queryByRole("button", { name: "Send again" })).toBeNull();
  const body = container.querySelector(".message-body")!;
  await user.pointer([{ keys: "[MouseLeft>]", target: body }, { keys: "[/MouseLeft]", target: body }]);
  expect(screen.queryByRole("region", { name: "About the selected text" })).toBeNull();
  expect(screen.queryByRole("menu")).toBeNull();
});

const cites: Message["cites"] = [{ page: "Ledger", revid: 1, span_from: 0, span_to: 5, chunk_id: "c-ledger-1" }];

test("the small button in the foot opens the message menu, also with the sources open", async () => {
  const user = userEvent.setup();
  const { container } = renderWith(<MessageView message={{ ...base, text: "The corridor closed.[#c-ledger-1]", cites }} showPlay={false} />);
  const foot = container.querySelector(".message-foot")!;
  const more = foot.querySelector("button.more") as HTMLButtonElement;
  // One row: sources, the button, the time.
  expect(foot.querySelector(".sources-toggle")).not.toBeNull();
  expect(more.compareDocumentPosition(foot.querySelector("time")!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  await user.click(more);
  expect(await screen.findByRole("menuitem", { name: "Copy text" })).toBeDefined();
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("menu")).toBeNull();
  await user.click(foot.querySelector(".sources-toggle")!);
  expect(container.querySelector(".sources-panel")).not.toBeNull();
  await user.click(foot.querySelector("button.more")!);
  expect(await screen.findByRole("menuitem", { name: "Copy text" })).toBeDefined();
});

test("the user's message shows one tick while a reply is under way and two when it is done", async () => {
  const channel = "dm:ticks";
  const mineMsg: Message = { ...base, id: "u1", channel, author: { kind: "user" }, text: "Hello there" };
  const { store, container } = renderWith(<MessageView message={mineMsg} showPlay={false} />);
  const ticks = () => container.querySelector(".ticks");
  await act(async () => {
    store.dispatch({ type: "messageAdded", event: { message: mineMsg } });
  });
  expect(ticks()).toBeNull();
  const author = { kind: "person", id: "mira" } as const;
  await act(async () => {
    store.dispatch({ type: "turnActivity", event: { channel, author, state: { kind: "thinking" } } });
  });
  expect(screen.getByRole("img", { name: "Delivered, a reply is coming" })).toBeDefined();
  await act(async () => {
    store.dispatch({ type: "messageAdded", event: { message: { ...base, id: "a1", channel, text: "Hi." } } });
    store.dispatch({ type: "turnActivity", event: { channel, author, state: { kind: "done" } } });
  });
  expect(screen.getByRole("img", { name: "Answered" })).toBeDefined();
});

test("offers sit under the bubble, inside its column", () => {
  const actions: Message["actions"] = [{ tool: "find_people", topic: "", persons: [{ id: "oren", name: "Oren" }] }];
  const { container } = renderWith(<MessageView message={{ ...base, text: "Try Oren.", actions }} showPlay={false} />);
  const stack = container.querySelector(".message-stack")!;
  expect(stack.querySelector(":scope > .choices")).not.toBeNull();
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

const mira = { kind: "person", id: "mira" } as const;

async function withHistory(channel: string, messages: Message[], names: Record<string, string> = { mira: "Mira" }) {
  const r = renderWith(<ChannelView channel={channel} />);
  await act(async () => {
    r.store.dispatch({ type: "names", persons: Object.entries(names).map(([id, name]) => ({ id, name })) });
    r.store.dispatch({ type: "history", channel, messages, older: false, done: true });
  });
  return r;
}

test("a reply shows an accent quote at the top of its bubble, and a click goes to the original", async () => {
  const original: Message = { ...base, id: "m1", text: "*She nods.*\nThe tea needs cooler water." };
  const answer: Message = { ...base, id: "m2", author: { kind: "user" }, text: "How cool?", reply: { id: "m1", author: mira, text: original.text, has_picture: false } };
  const { container } = await withHistory("dm:mira", [original, answer]);
  const quote = container.querySelector<HTMLButtonElement>("#m-m2 .quote")!;
  // Author's name and the first spoken line, not the scene line; before the reply's own words.
  expect(quote.querySelector(".quote-name")!.textContent).toBe("Mira");
  expect(quote.querySelector(".quote-text")!.textContent).toBe("The tea needs cooler water.");
  expect(quote.className).toMatch(/hue-\d/);
  const bubble = quote.closest(".bubble")!;
  expect(bubble.firstElementChild).toBe(quote);
  expect(quote.compareDocumentPosition(bubble.querySelector(".speech")!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  await userEvent.setup().click(quote);
  expect(container.querySelector("#m-m1")!.classList.contains("flash")).toBe(true);
});

test("a quote of a picture says so when the original had no words", async () => {
  const answer: Message = { ...base, id: "m2", author: { kind: "user" }, text: "Nice.", reply: { id: "m1", author: mira, text: "", has_picture: true } };
  const { container } = await withHistory("dm:mira", [answer]);
  expect(container.querySelector(".quote-text")!.textContent).toBe("Picture");
});

test("a forwarded copy names its original author above the words", async () => {
  const copy: Message = { ...base, id: "m3", author: { kind: "user" }, text: "Keep to the lit streets.", forwarded_from: mira };
  const { container } = await withHistory("dm:oren", [{ ...copy, channel: "dm:oren" }]);
  const header = container.querySelector(".forwarded")!;
  expect(header.textContent).toBe("Forwarded from Mira");
  expect(header.compareDocumentPosition(container.querySelector(".speech")!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});

test("the context menu lists reply, copy, forward, send again for one's own, sources, in that order", async () => {
  const user = userEvent.setup();
  const cites: Message["cites"] = [{ page: "Ledger", revid: 1, span_from: 0, span_to: 5, chunk_id: "c-ledger-1" }];
  const { container } = await withHistory("dm:mira", [
    { ...base, id: "m1", text: "Hello.", cites },
    { ...base, id: "m2", author: { kind: "user" }, text: "Mine." },
  ]);
  const labels = async (id: string) => {
    await user.pointer({ keys: "[MouseRight]", target: container.querySelector(`#m-${id}`)! });
    const menu = await screen.findByRole("menu", { name: "Actions for this message" });
    const out = within(menu).getAllByRole("menuitem").map((i) => i.textContent);
    await user.keyboard("{Escape}");
    return out;
  };
  expect(await labels("m1")).toEqual(["Reply", "Copy text", "Forward", "Sources (1)"]);
  expect(await labels("m2")).toEqual(["Reply", "Copy text", "Forward", "Send again"]);
});

test("Reply from the menu opens the reply bar and the focus goes to the box", async () => {
  const user = userEvent.setup();
  const { container, store } = await withHistory("dm:mira", [{ ...base, id: "m1", text: "Hello." }]);
  await user.pointer({ keys: "[MouseRight]", target: container.querySelector("#m-m1")! });
  await user.click(await screen.findByRole("menuitem", { name: "Reply" }));
  expect(store.getState().replies["dm:mira"].id).toBe("m1");
  expect(screen.getByRole("group", { name: "Replying to Mira" })).toBeDefined();
  expect(document.activeElement).toBe(screen.getByRole("textbox", { name: "Message" }));
});

test("a focused message answers keys: R replies, F opens the forward picker, the menu key opens the menu", async () => {
  const user = userEvent.setup();
  const { container, store } = await withHistory("dm:mira", [{ ...base, id: "m1", text: "Hello." }]);
  const article = container.querySelector<HTMLElement>("#m-m1")!;
  article.focus();
  await user.keyboard("f");
  expect(store.getState().forwarding?.id).toBe("m1");
  act(() => store.dispatch({ type: "forwarding", message: null }));
  article.focus();
  await user.keyboard("{ContextMenu}");
  expect(await screen.findByRole("menu", { name: "Actions for this message" })).toBeDefined();
  await user.keyboard("{Escape}");
  article.focus();
  await user.keyboard("r");
  expect(store.getState().replies["dm:mira"].id).toBe("m1");
});

test("@name of a member is set apart in a group message, and only there", async () => {
  const r = renderWith(<ChannelView channel="g-rota" />);
  await act(() => r.actions.refreshChannels());
  await act(async () => {
    r.store.dispatch({
      type: "history",
      channel: "g-rota",
      messages: [{ ...base, id: "m1", channel: "g-rota", author: { kind: "user" }, text: "@Oren Lark can you take nights? Not @Nobody." }],
      older: false,
      done: true,
    });
  });
  const marks = [...r.container.querySelectorAll(".mention")].map((m) => m.textContent);
  expect(marks).toEqual(["@Oren Lark"]);
  r.unmount();

  const direct = await withHistory("dm:mira", [{ ...base, id: "m2", author: { kind: "user" }, text: "@Mira how are you?" }]);
  expect(direct.container.querySelector(".mention")).toBeNull();
});
