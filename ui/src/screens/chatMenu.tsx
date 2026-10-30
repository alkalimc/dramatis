import { useCallback, useState } from "react";
import type { ChannelSummary } from "../api";
import { useT } from "../i18n";
import { unreadOf } from "../lib/chats";
import { useActions, useApi, useStore } from "../store";
import { Menu, type MenuItem } from "../ui/Menu";

/**
 * The actions on one conversation, as menu items: the same list from a right click on a
 * row and from the header's menu. Clearing history is left out: the engine has no such
 * command, and taking everything back is stronger than that.
 */
export function useChatMenu() {
  const t = useT();
  const api = useApi();
  const actions = useActions();
  const pinned = useStore((s) => s.pinned);
  const unread = useStore((s) => s.unread);
  const [open, setOpen] = useState<{ channel: ChannelSummary; x: number; y: number } | null>(null);

  const items = useCallback(
    (c: ChannelSummary): MenuItem[] => {
      const out: MenuItem[] = [];
      const isPinned = pinned.includes(c.id);
      out.push({ label: t(isPinned ? "chatmenu.unpin" : "chatmenu.pin"), icon: "pin", onSelect: () => actions.pin(c.id, !isPinned) });
      if (unreadOf(c, unread) > 0) out.push({ label: t("chatmenu.read"), icon: "check", onSelect: () => void actions.clearUnread(c.id) });
      else out.push({ label: t("chatmenu.unread"), icon: "unread", onSelect: () => actions.markUnread(c.id) });
      if (c.kind !== "host") {
        const archived = c.archived;
        out.push({ label: t(archived ? "chatmenu.unarchive" : "chatmenu.archive"), icon: "archive", onSelect: () => void actions.archive(c.id, !archived) });
      }
      if (c.kind === "group") {
        out.push({
          label: t("channel.delete"),
          icon: "trash",
          danger: true,
          separated: true,
          onSelect: async () => {
            if (!window.confirm(t("channel.delete.confirm"))) return;
            if ((await actions.attempt(api.commands.deleteGroup(c.id))) === undefined) return;
            await actions.refreshChannels();
            await actions.open(null);
          },
        });
      }
      return out;
    },
    [t, api, actions, pinned, unread],
  );

  const close = useCallback(() => setOpen(null), []);
  const menu = open && <Menu at={open} items={items(open.channel)} label={t("chatmenu.label")} onClose={close} />;
  return { items, menu, show: (channel: ChannelSummary, x: number, y: number) => setOpen({ channel, x, y }) };
}

