/** Small line icons, drawn inline so they follow the text colour and need no assets. */
const PATHS = {
  chats: "M4 5h16v11H9l-5 4z",
  people: "M9 11a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM3 20c0-3.3 2.7-6 6-6s6 2.7 6 6M16 6a3 3 0 0 1 0 5.5M18 14.3c1.8.8 3 2.6 3 4.7",
  cases: "M5 4h14v16H5zM9 9h6M9 13h6M9 17h3",
  world: "M12 3l2.6 5.6 6.1.7-4.5 4.2 1.2 6L12 16.4 6.6 19.5l1.2-6L3.3 9.3l6.1-.7z",
  system: "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM19 12a7 7 0 0 0-.1-1.3l2-1.5-2-3.4-2.3 1a7 7 0 0 0-2.2-1.3L14 3h-4l-.4 2.5A7 7 0 0 0 7.4 6.8l-2.3-1-2 3.4 2 1.5a7 7 0 0 0 0 2.6l-2 1.5 2 3.4 2.3-1c.7.6 1.4 1 2.2 1.3L10 21h4l.4-2.5a7 7 0 0 0 2.2-1.3l2.3 1 2-3.4-2-1.5c.1-.4.1-.9.1-1.3z",
  attach: "M20 11.5l-8 8a5 5 0 0 1-7-7l9-9a3.3 3.3 0 0 1 4.7 4.7l-9 9a1.7 1.7 0 0 1-2.4-2.4l8-8",
  send: "M4 12l16-8-6 16-3-7z",
  more: "M12 5.5v.01M12 12v.01M12 18.5v.01",
  panel: "M4 5h16v14H4zM14 5v14",
  group: "M7 9a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5zM17 9a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5zM2 19c0-2.8 2.2-5 5-5s5 2.2 5 5M12.5 14.2c.8-.2 1.6-.2 2.5-.2 2.8 0 5 2.2 5 5",
  menu: "M4 7h16M4 12h16M4 17h16",
  search: "M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14zM20 20l-4-4",
  close: "M6 6l12 12M18 6L6 18",
  back: "M15 5l-7 7 7 7",
  archive: "M4 5h16v4H4zM6 9v10h12V9M10 13h4",
  pin: "M9 4h6l-1 6 3 3H7l3-3zM12 13v7",
  check: "M5 12.5l4.5 4.5L19 7.5",
  trash: "M5 7h14M10 7V4h4v3M7 7l1 13h8l1-13",
  camera: "M4 8h3l2-3h6l2 3h3v11H4zM12 17a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7z",
  moon: "M20 14.5A8 8 0 1 1 9.5 4 6.5 6.5 0 0 0 20 14.5z",
  pencil: "M5 19l1-4L16.5 4.5a2 2 0 0 1 3 3L9 18zM14.5 6.5l3 3",
  user: "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 20c0-4 3.6-6 8-6s8 2 8 6",
  bookmark: "M7 4h10v16l-5-4-5 4z",
  plus: "M12 5v14M5 12h14",
  unread: "M12 16a4 4 0 1 0 0-8 4 4 0 0 0 0 8z",
} as const;

export type IconName = keyof typeof PATHS;

/** Dots read as dots only with a thicker pen. */
const HEAVY: IconName[] = ["more"];

export function Icon({ name, size = 22 }: { name: IconName; size?: number }) {
  return (
    <svg className="icon-svg" viewBox="0 0 24 24" width={size} height={size} fill="none" stroke="currentColor" strokeWidth={HEAVY.includes(name) ? 3 : 1.7} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false">
      <path d={PATHS[name]} />
    </svg>
  );
}
