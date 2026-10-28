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
  const sent = await api.commands.sendMessage("host", "Who can help with ledgers?", null);
  expect(sent.status).toBe("ok");
  const log = await screen.findByRole("log");
  await within(log).findByText("Who can help with ledgers?");
  await waitFor(() => expect(store.getState().typing.host).toBeUndefined());
  const before = store.getState().messages.host.length;
  vi.spyOn(window, "confirm").mockReturnValue(true);
  const mine = within(log).getAllByRole("article").find((a) => a.className.includes("from-user") && a.textContent?.includes("Who can help"))!;
  await user.click(within(mine).getByRole("button", { name: "Send again" }));
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
