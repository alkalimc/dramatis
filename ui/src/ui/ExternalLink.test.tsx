import { fireEvent, screen } from "@testing-library/react";
import { createStore, createActions, Provider } from "../store";
import { render } from "@testing-library/react";
import { mockApi } from "../test/render";
import { ExternalLink } from "./ExternalLink";

function show(mock: boolean, opened: string[]) {
  const base = mockApi();
  const api = {
    ...base,
    mock,
    commands: {
      ...base.commands,
      openExternal: async (url: string) => {
        opened.push(url);
        return { status: "ok" as const, data: null };
      },
    },
  };
  const store = createStore();
  render(
    <Provider store={store} api={api} actions={createActions(api, store)}>
      <ExternalLink href="https://wiki.example/x">View original</ExternalLink>
    </Provider>,
  );
  return screen.getByRole("link", { name: "View original" });
}

test("in the app the address goes to the system browser, not to the webview", () => {
  const opened: string[] = [];
  const link = show(false, opened);
  expect(link.getAttribute("href")).toBe("https://wiki.example/x");
  // A cancelled click is the sign the engine took it.
  expect(fireEvent.click(link)).toBe(false);
  expect(opened).toEqual(["https://wiki.example/x"]);
});

test("in a plain browser the link works as written", () => {
  const opened: string[] = [];
  const link = show(true, opened);
  expect(link.getAttribute("target")).toBe("_blank");
  link.addEventListener("click", (e) => e.preventDefault());
  fireEvent.click(link);
  expect(opened).toEqual([]);
});
