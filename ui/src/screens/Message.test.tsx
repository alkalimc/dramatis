import { screen } from "@testing-library/react";
import type { Message } from "../api";
import { renderWith } from "../test/render";
import { MessageView } from "./Message";

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
