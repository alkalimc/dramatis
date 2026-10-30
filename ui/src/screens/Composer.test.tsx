import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderWith } from "../test/render";
import { subscribe } from "../store";
import { ChannelView } from "./Channel";

async function mounted() {
  const r = renderWith(<ChannelView channel="host" />);
  await act(() => r.actions.loadHistory("host"));
  return r;
}

const box = () => screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement;

test("Enter adds a line and sends nothing; Cmd+Enter and Ctrl+Enter send", async () => {
  const user = userEvent.setup();
  const { api } = await mounted();
  const sent = vi.spyOn(api.commands, "sendMessage");
  await user.type(box(), "first{Enter}second");
  expect(box().value).toBe("first\nsecond");
  expect(sent).not.toHaveBeenCalled();
  await user.keyboard("{Meta>}{Enter}{/Meta}");
  expect(sent).toHaveBeenCalledTimes(1);
  expect(sent.mock.calls[0][1]).toBe("first\nsecond");
  await waitFor(() => expect(box().value).toBe(""));
  await user.type(box(), "again");
  await user.keyboard("{Control>}{Enter}{/Control}");
  await waitFor(() => expect(sent).toHaveBeenCalledTimes(2));
});

test("an Enter that commits an input-method candidate never sends", async () => {
  const user = userEvent.setup();
  const { api } = await mounted();
  const sent = vi.spyOn(api.commands, "sendMessage");
  await user.type(box(), "ni");
  fireEvent.keyDown(box(), { key: "Enter", isComposing: true });
  fireEvent.keyDown(box(), { key: "Enter", keyCode: 229 });
  fireEvent.keyDown(box(), { key: "Enter", metaKey: true, isComposing: true });
  expect(sent).not.toHaveBeenCalled();
  expect(box().value).toBe("ni");
});

test("the shortcut is shown next to the box", async () => {
  await mounted();
  expect(screen.getByText(/Enter adds a line, .*Enter sends/)).toBeDefined();
});

test("Send again takes a message and what followed back, and puts its text in the box", async () => {
  const user = userEvent.setup();
  const { api, store, actions } = await mounted();
  // The echo and the take-back arrive as events, as they do in the app.
  await subscribe(api, store, actions);
  const sent = await api.commands.sendMessage("host", "Who can help with ledgers?", null, null);
  expect(sent.status).toBe("ok");
  const log = await screen.findByRole("log");
  await within(log).findByText("Who can help with ledgers?");
  await waitFor(() => expect(store.getState().typing.host).toBeUndefined());
  const before = store.getState().messages.host.length;
  vi.spyOn(window, "confirm").mockReturnValue(true);
  const mine = within(log).getAllByRole("article").find((a) => a.className.includes("from-user") && a.textContent?.includes("Who can help"))!;
  await user.pointer({ keys: "[MouseRight]", target: mine });
  await user.click(await screen.findByRole("menuitem", { name: "Send again" }));
  await waitFor(() => expect(box().value).toBe("Who can help with ledgers?"));
  expect(store.getState().messages.host.length).toBeLessThan(before);
  expect(within(log).queryByText("Who can help with ledgers?")).toBeNull();
  expect(store.getState().restore).toBeNull();
});

test("a send that fails leaves nothing waiting, and keeps the draft", async () => {
  const user = userEvent.setup();
  const { api, store } = await mounted();
  vi.spyOn(api.commands, "sendMessage").mockResolvedValue({ status: "error", error: { code: "no_endpoint" } });
  await user.type(box(), "hello");
  await user.keyboard("{Control>}{Enter}{/Control}");
  await waitFor(() => expect(store.getState().error?.code).toBe("no_endpoint"));
  expect(store.getState().awaiting.host).toBeUndefined();
  expect(box().value).toBe("hello");
});

test("a picture pasted into the box becomes a removable preview and is sent with the text", async () => {
  const user = userEvent.setup();
  const { api } = await mounted();
  const sent = vi.spyOn(api.commands, "sendMessage");
  const file = new File([new Uint8Array([137, 80, 78, 71])], "shot.png", { type: "image/png" });
  fireEvent.paste(box(), { clipboardData: { items: [{ kind: "file", type: "image/png", getAsFile: () => file }], files: [file] } });
  const preview = await screen.findByRole("img", { name: "Attached image" });
  expect(preview.getAttribute("src")).toMatch(/^data:image\/png;base64,/);
  await user.click(screen.getByRole("button", { name: "Remove image" }));
  expect(screen.queryByRole("img", { name: "Attached image" })).toBeNull();

  fireEvent.paste(box(), { clipboardData: { items: [{ kind: "file", type: "image/png", getAsFile: () => file }], files: [file] } });
  await screen.findByRole("img", { name: "Attached image" });
  await user.type(box(), "look");
  await user.keyboard("{Meta>}{Enter}{/Meta}");
  await waitFor(() => expect(sent).toHaveBeenCalledTimes(1));
  expect(sent.mock.calls[0][2]).toMatchObject({ mime: "image/png" });
});

test("pasting plain text is left alone", async () => {
  await mounted();
  const ev = fireEvent.paste(box(), { clipboardData: { items: [{ kind: "string", type: "text/plain", getAsFile: () => null }], files: [] } });
  expect(ev).toBe(true);
  expect(screen.queryByRole("img", { name: "Attached image" })).toBeNull();
});

test("the paperclip opens the image picker at once", async () => {
  const user = userEvent.setup();
  await mounted();
  const input = document.querySelector<HTMLInputElement>('input[type="file"]')!;
  const picker = vi.spyOn(input, "click");
  await user.click(screen.getByRole("button", { name: "Attach" }));
  expect(picker).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole("menu")).toBeNull();
});

test("hovering the paperclip lists the kinds that work, and the item opens the picker", async () => {
  const user = userEvent.setup();
  await mounted();
  const input = document.querySelector<HTMLInputElement>('input[type="file"]')!;
  const picker = vi.spyOn(input, "click");
  expect(screen.queryByRole("menu", { name: "Attach" })).toBeNull();
  await user.hover(screen.getByRole("button", { name: "Attach" }));
  const tray = await screen.findByRole("menu", { name: "Attach" });
  const items = within(tray).getAllByRole("menuitem");
  expect(items.map((i) => i.textContent)).toEqual(["Picture"]);
  fireEvent.click(items[0]);
  expect(picker).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole("menu", { name: "Attach" })).toBeNull();
});

test("the tray closes when the pointer leaves", async () => {
  const user = userEvent.setup();
  await mounted();
  await user.hover(screen.getByRole("button", { name: "Attach" }));
  await screen.findByRole("menu", { name: "Attach" });
  await user.unhover(screen.getByRole("button", { name: "Attach" }));
  expect(screen.queryByRole("menu", { name: "Attach" })).toBeNull();
});

test("the tray is reachable from the keyboard: focus shows it, the item opens the picker", async () => {
  const user = userEvent.setup();
  await mounted();
  const input = document.querySelector<HTMLInputElement>('input[type="file"]')!;
  const picker = vi.spyOn(input, "click");
  screen.getByRole("button", { name: "Attach" }).focus();
  await screen.findByRole("menu", { name: "Attach" });
  await user.tab();
  await user.keyboard("{Enter}");
  expect(picker).toHaveBeenCalledTimes(1);
});

test("a reply shows who and what it answers above the box, goes with the send, and ends there", async () => {
  const user = userEvent.setup();
  const { api, store } = await mounted();
  const sent = vi.spyOn(api.commands, "sendMessage");
  const quoted = store.getState().messages.host.find((m) => m.author.kind === "host")!;
  act(() => store.dispatch({ type: "reply", channel: "host", message: quoted }));
  const bar = screen.getByRole("group", { name: /Replying to/ });
  expect(bar.textContent).toContain(quoted.text.split("\n")[0].slice(0, 20));
  await user.type(box(), "and then?");
  await user.keyboard("{Control>}{Enter}{/Control}");
  await waitFor(() => expect(sent).toHaveBeenCalledTimes(1));
  expect(sent.mock.calls[0][3]).toBe(quoted.id);
  await waitFor(() => expect(screen.queryByRole("group", { name: /Replying to/ })).toBeNull());
  expect(store.getState().replies.host).toBeUndefined();
  // The next one answers nothing.
  await user.type(box(), "plain");
  await user.keyboard("{Control>}{Enter}{/Control}");
  await waitFor(() => expect(sent).toHaveBeenCalledTimes(2));
  expect(sent.mock.calls[1][3]).toBeNull();
});

test("the close button and Escape drop the reply", async () => {
  const user = userEvent.setup();
  const { store } = await mounted();
  const quoted = store.getState().messages.host[0];
  act(() => store.dispatch({ type: "reply", channel: "host", message: quoted }));
  await user.click(screen.getByRole("button", { name: "Cancel the reply" }));
  expect(screen.queryByRole("group", { name: /Replying to/ })).toBeNull();
  act(() => store.dispatch({ type: "reply", channel: "host", message: quoted }));
  box().focus();
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("group", { name: /Replying to/ })).toBeNull();
});

test("a failed send keeps the reply and the draft", async () => {
  const user = userEvent.setup();
  const { api, store } = await mounted();
  vi.spyOn(api.commands, "sendMessage").mockResolvedValue({ status: "error", error: { code: "no_endpoint" } });
  const quoted = store.getState().messages.host[0];
  act(() => store.dispatch({ type: "reply", channel: "host", message: quoted }));
  await user.type(box(), "hello");
  await user.keyboard("{Control>}{Enter}{/Control}");
  await waitFor(() => expect(store.getState().error?.code).toBe("no_endpoint"));
  expect(store.getState().replies.host?.id).toBe(quoted.id);
});

async function inGroup() {
  const r = renderWith(<ChannelView channel="g-rota" />);
  await act(() => r.actions.refreshChannels());
  await act(() => r.actions.loadHistory("g-rota"));
  return r;
}

test("typing @ in a group lists its members, filtered as you type; Enter puts @name in the text", async () => {
  const user = userEvent.setup();
  const { api } = await inGroup();
  const sent = vi.spyOn(api.commands, "sendMessage");
  await user.type(box(), "hi @");
  const list = await screen.findByRole("listbox", { name: "Members to mention" });
  const shown = () => within(list).getAllByRole("option").map((o) => o.querySelector("button > span:last-child")!.textContent);
  expect(shown()).toEqual(["Oren Lark", "Mira Vale", "Tess Arden"]);
  await user.type(box(), "mi");
  expect(shown()).toEqual(["Mira Vale"]);
  await user.keyboard("{Enter}");
  expect(box().value).toBe("hi @Mira Vale ");
  expect(screen.queryByRole("listbox")).toBeNull();
  // The text goes as typed: the engine finds the member's name in it.
  await user.keyboard("are you there?{Control>}{Enter}{/Control}");
  await waitFor(() => expect(sent).toHaveBeenCalledTimes(1));
  expect(sent.mock.calls[0][1]).toBe("hi @Mira Vale are you there?");
});

test("arrows move through the members, a click picks one, Escape closes the list", async () => {
  const user = userEvent.setup();
  await inGroup();
  await user.type(box(), "@");
  const list = await screen.findByRole("listbox");
  await user.keyboard("{ArrowDown}");
  expect(within(list).getAllByRole("option").map((o) => o.getAttribute("aria-selected"))).toEqual(["false", "true", "false"]);
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("listbox")).toBeNull();
  expect(box().value).toBe("@");
  await user.type(box(), "t");
  await user.click(within(await screen.findByRole("listbox")).getByRole("button", { name: "Tess Arden" }));
  expect(box().value).toBe("@Tess Arden ");
});

test("a direct chat has no member list", async () => {
  const user = userEvent.setup();
  await mounted();
  await user.type(box(), "@");
  expect(screen.queryByRole("listbox")).toBeNull();
});

test("the favorites take notes: a different prompt, and nothing is awaited", async () => {
  const user = userEvent.setup();
  const r = renderWith(<ChannelView channel="favorites" />);
  await subscribe(r.api, r.store, r.actions);
  await act(() => r.actions.refreshChannels());
  await act(() => r.actions.loadHistory("favorites"));
  const field = screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement;
  expect(field.placeholder).toBe("Write a note...");
  await user.type(field, "remember the stairs");
  await user.keyboard("{Control>}{Enter}{/Control}");
  await waitFor(() => expect(r.store.getState().messages.favorites.some((m) => m.text === "remember the stairs")).toBe(true));
  expect(r.store.getState().awaiting.favorites).toBeUndefined();
});
