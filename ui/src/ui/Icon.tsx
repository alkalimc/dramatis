/** Small line icons, drawn inline so they follow the text colour and need no assets. */
const PATHS = {
  chats: "M4 5h16v11H9l-5 4z",
  people: "M9 11a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM3 20c0-3.3 2.7-6 6-6s6 2.7 6 6M16 6a3 3 0 0 1 0 5.5M18 14.3c1.8.8 3 2.6 3 4.7",
  cases: "M5 4h14v16H5zM9 9h6M9 13h6M9 17h3",
  world: "M12 3l2.6 5.6 6.1.7-4.5 4.2 1.2 6L12 16.4 6.6 19.5l1.2-6L3.3 9.3l6.1-.7z",
  system: "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM19 12a7 7 0 0 0-.1-1.3l2-1.5-2-3.4-2.3 1a7 7 0 0 0-2.2-1.3L14 3h-4l-.4 2.5A7 7 0 0 0 7.4 6.8l-2.3-1-2 3.4 2 1.5a7 7 0 0 0 0 2.6l-2 1.5 2 3.4 2.3-1c.7.6 1.4 1 2.2 1.3L10 21h4l.4-2.5a7 7 0 0 0 2.2-1.3l2.3 1 2-3.4-2-1.5c.1-.4.1-.9.1-1.3z",
  attach: "M20 11.5l-8 8a5 5 0 0 1-7-7l9-9a3.3 3.3 0 0 1 4.7 4.7l-9 9a1.7 1.7 0 0 1-2.4-2.4l8-8",
  send: "M4 12l16-8-6 16-3-7z",
  more: "M12 6h.01M12 12h.01M12 18h.01",
  panel: "M4 5h16v14H4zM14 5v14",
  group: "M7 9a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5zM17 9a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5zM2 19c0-2.8 2.2-5 5-5s5 2.2 5 5M12.5 14.2c.8-.2 1.6-.2 2.5-.2 2.8 0 5 2.2 5 5",
} as const;

export type IconName = keyof typeof PATHS;

export function Icon({ name }: { name: IconName }) {
  return (
    <svg className="icon-svg" viewBox="0 0 24 24" width="22" height="22" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false">
      <path d={PATHS[name]} />
    </svg>
  );
}
