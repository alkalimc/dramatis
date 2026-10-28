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
