"""In-world phrasing for runtime surfaces.

The client renders these; it does not compose them. Keeping the wording in the pack
rather than in the client means a second domain re-skins the product by editing one
file, and — more immediately — it means the client cannot quietly invent phrasing
that leaks the system's own vocabulary into the fiction.

The rule these follow: **change the narration, never the capability.** An approval
prompt worded in-world still asks for the same approval, and a confidence warning
worded in-world still says the archive is thin. Dressing up a refusal as a character
choice would be a lie; dressing up a *true* statement in the world's register is
just presentation.
"""

from __future__ import annotations

WORDING: dict[str, str] = {
    # the assistant
    "assistant.name": "PRTS",
    "assistant.role": "罗德岛作战协议终端",
    "user.address": "博士",
    # retrieval confidence, said in-world
    "confidence.low": "档案里没有确切记载",
    "confidence.medium": "档案有记载，但不完整",
    "citation.label": "档案出处",
    "citation.stale": "本条抓取于早前版本，线上可能已更新",
    # presence
    "presence.busy": "正在处理你交办的事",
    "presence.meeting": "在会议中",
    "presence.asleep": "这个时候通常在休息",
    "presence.away": "不在岛上",
    # work
    "task.accepted": "已接下",
    "task.incomplete": "未完成的草稿",
    "task.overdue": "已过期限",
    "task.ungrounded": "资料不足，结论存疑",
    "archive.room": "档案室",
    # approval, worded in-world without softening what is being asked
    "approval.write": "需要你签字才能改动这份文件",
    "approval.exec": "需要你授权才能执行这条指令",
    "approval.network": "需要你授权才能对外查询",
    # cases
    "case.open": "在办",
    "case.cold": "搁置",
    "case.closed": "已结案",
    "evidence.conflict": "两份档案说法不一致",
}
