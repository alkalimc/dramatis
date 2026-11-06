# Plugin interface

A plugin is a separate program that the app starts and talks to in lines of JSON. It can
add lines to the turn the model is about to answer, rewrite a tool's result before the model
reads it, offer tools of its own, and watch the messages that are posted. It cannot read the
API keys, and a plugin that fails never breaks a turn.

The example in `examples/plugins/dice/` (Python) uses every hook; copy it to start.

## Install and enable

A plugin is a folder in the plugins folder of the app's home (`$DRAMATIS_HOME/plugins/`, by
default `~/.dramatis/plugins/`):

```text
plugins/
  enabled.toml        written by the app: enabled = ["dice"]
  dice/
    plugin.toml       the manifest
    dice.py           whatever the manifest runs
```

Plugins are **off** until the user turns them on in Settings, under Plugins. The list shows
each plugin's hooks and tools, why a manifest is unusable, and the last failure of a running
plugin. Turning a plugin off ends its process. The folder name must equal the manifest's
`name`.

## Manifest

```toml
name = "dice"                       # a-z, 0-9, "-"; 1 to 32 characters; equals the folder name
version = "0.1.0"
description = "Shown in Settings."
protocol = 1                        # the only version
command = ["python3", "dice.py"]    # argv, no shell; use "python" on Windows
hooks = ["prompt", "tool_result", "message"]
timeout_ms = 1000                   # per call; default 1000, from 50 to 5000

[[tools]]                           # up to 8
name = "roll"                       # a-z, 0-9, "_", starts with a letter, no "__"
description = "Roll dice for the user."   # required, up to 1000 characters
audience = "all"                    # "all" (default), "host" or "persons"

[tools.parameters]                  # a JSON Schema for the arguments, written as TOML
type = "object"
required = ["sides"]
[tools.parameters.properties.sides]
type = "integer"
```

- `command` runs with the plugin's folder as working directory. A first word with a path
  separator (`./run.sh`) is a file inside the folder and may not leave it; any other first
  word is looked up on `PATH`.
- Unknown keys are ignored. An unknown hook name makes the manifest invalid.
- Tools are declared here, not at run time, so Settings can show a plugin's reach before it
  is turned on and the tool list never depends on a running process.
- The model sees a tool as `<plugin>__<tool>` (`dice__roll`).

## Process and protocol

The app starts the plugin on first use and keeps it running while it is enabled. Messages
are single lines of UTF-8 JSON, one object per line, ended by a newline, on the plugin's
stdin (from the app) and stdout (from the plugin). **Stdout is for the protocol only**; write
logs to stderr, which the app saves in `$DRAMATIS_HOME/logs/plugin-<name>.log` (truncated each
time the plugin starts).

A request has an `id` (an integer, unique per process) and expects exactly one reply with the
same `id`:

```json
{"id": 2, "method": "prompt", "params": {"channel": {"id": 4, "kind": "direct"}, "persons": ["ann"], "text": "hello"}}
{"id": 2, "result": {"lines": ["It is raining."]}}
{"id": 2, "error": {"message": "no weather service"}}
```

A notification has no `id` and gets no reply:

```json
{"method": "message", "params": {"id": 17, "channel": 4, "author": {"kind": "user"}, "text": "hello", "service": false, "at": "2026-03-05T12:00:00+00:00"}}
```

A reply that is not one JSON object, has another `id`, or arrives as a line over 64 KiB is a
failure (below). A request that would be over 48 KiB is not sent and counts as a failure too; the texts the app puts in requests are cut well below that.

### Handshake

Right after starting, the app sends `initialize` and waits up to 3 seconds for any `result`:

```json
{"id": 1, "method": "initialize", "params": {"protocol": 1, "app": "dramatis", "version": "1.1.1", "plugin": "dice"}}
{"id": 1, "result": {}}
```

### Environment

The process gets an empty environment plus `PATH`, `PATHEXT`, `SystemRoot`, `TEMP`, `TMP`,
`TMPDIR`, `LANG`, `LC_ALL`, and two of its own: `DRAMATIS_PLUGIN_DIR` and
`DRAMATIS_PLUGIN_NAME`. Nothing else of the app's environment is passed on.

## Hooks

Every call is bounded by the plugin's `timeout_ms`. Calls into one plugin are made one at a
time. All limits are applied by the app after the reply; a plugin cannot raise them.

### `prompt`: lines for a turn

When the user posts a message that starts a turn (a direct chat, a group, or the host), the
app asks each enabled plugin that takes `prompt` for lines to add to that turn's entry.

Params: `channel` (`id`, and `kind`: `direct`, `group` or `host`), `persons` (roster ids of
the people in the channel) and `text` (the user's message, cut to 8000 characters).
Result: `{"lines": ["...", "..."]}`.

The lines join the turn's per-turn harness lines (the same entry that holds the date and
time line), after the user's words. They are never added to the segment's cached prefix.
That prefix (the instruction blocks and the tool list) is byte-identical for the life of a
conversation segment so that the endpoint can reuse its cache; lines that change from turn to
turn would invalidate it for every later call. Appended to the turn's own entry, they cost
only their own tokens.

Limits: up to 5 lines per plugin; each line is flattened to one line (control characters
become spaces), trimmed and cut to 400 characters; 1500 characters in total across all
plugins. At most 4 seconds in total are spent on the plugins for one turn; later plugins are
skipped once it is used up. Lines are written into the log: they stay in the conversation
for its life, and the model reads them as harness text, not as something the user said.

Only turns the user starts are covered in version 1. Openings, interjections, reminders, and
wrap-ups get no `prompt` call.

### `tool_result`: post-process a result

After any built-in or plugin tool runs, and before the model reads its output, each enabled
plugin that takes `tool_result` is called in name order, each seeing the output as the one
before it left it.

Params: `tool` (the name as the model called it), `arguments` (the model's arguments, parsed
as JSON; the raw text if they do not parse), `output` (the text, cut to 8000 characters) and
`context` (`channel`, `persons` as above).
Result: `{}` to leave the output, or `{"output": "new text"}` to replace it (cut to 8000
characters).

To observe only, answer `{}`. Replacing output is the plugin's responsibility: for example,
dropping a passage id such as `[#u2]` from a search result means the model cannot cite it.
The wrap-up turn is not passed through this hook. The result is stored as the plugin left
it.

### `tool`: tools of its own

A tool declared in the manifest is offered to the model after the built-in ones, in plugin
name order and then declaration order, to the sessions its `audience` names (`host` is the
host's session, `persons` the people's, `all` both). When the model calls it, the app sends:

```json
{"id": 9, "method": "tool", "params": {"tool": "roll", "arguments": {"sides": 6}, "context": {"channel": {"id": 4, "kind": "direct"}, "persons": ["ann"]}}}
{"id": 9, "result": {"output": "4 (total 4)"}}
```

`tool` is the name without the plugin prefix. `output` is required and is cut to 8000
characters. An `error` reply goes to the model as a refusal that carries the message (cut to
300 characters), in the corpus's own words. Any other failure, and a call to a plugin that is
off or stopped, goes to the model as "not available". Plugin tools are not run in wrap-up
turns. The `tool_result` hook sees plugin tools' output like any other.

The tool list is part of the cached prefix. Turning a plugin with tools on or off therefore
ends the current conversation segments on their next turn: each is wrapped up with a summary
and a new one opens with the new list (one extra model call per active conversation, then
the cache builds again). Plugins without tools never cause this.

### `message`: watch posted messages

For every message added to any channel (the user's, the host's and people's, service notes
included), a `message` notification is queued for each enabled plugin that takes it. It is
never waited for: a queue of 256 holds the events for a slow plugin, and when it is full new
events are dropped. Delivery is in order, once, with no reply.

Params: `id`, `channel` (numbers), `author` (`{"kind": "user"}`, `{"kind": "host"}` or
`{"kind": "person", "id": "ann"}`), `text` (cut to 8000 characters), `service` (a harness
note, not something said) and `at` (the message's timestamp). Pictures are not sent.

### Not in version 1

Daily-event sources and hooks on the host's own actions are left out until the
event pipeline settles. Hooks are added under new names; `protocol` changes only if an
existing message changes.

## Failure handling

A call fails when the process cannot start, does not answer in time, exits, closes its pipe,
or sends a bad or oversized line. The call then contributes nothing (no lines, the output
unchanged, "not available" for a tool) and the turn goes on. The process is killed and is
started afresh at the next call. After 3 failures in a row the plugin is **stopped**: it is
no longer called until the user turns it off and on again, and Settings shows the reason. An
`error` reply is not a failure of the process and does not count.

The calls are made on worker threads, never on the app's UI thread, and nothing waits on a
plugin after its timeout.

## Security model

- Plugins are separate programs. The app does not link or load plugin code.
- API keys live in the system keychain and are read only by the app's own code. A plugin has
  no way to ask for them, and the environment it receives carries none.
- A plugin is **not sandboxed**. It runs as the user, with the user's file and network
  access, and can do anything the user can. Turn on only plugins you trust, and read what a
  plugin does with the messages it receives; Settings says this where plugins are turned on.
- What a plugin is sent depends on its hooks: `prompt` and `message` carry what the user and
  the people write; `tool_result` carries tool arguments and results, which include archive
  passages and the user's memories. A plugin that takes none of these and offers no tools is
  never called.
- A plugin influences the model only through the three channels above (lines, results,
  tools), each bounded as described. It cannot change the cached prefix, the instruction
  blocks or the world's data directly.
- Plugins are off by default, and discovery only reads manifests; nothing starts until the
  plugin is on and a hook needs it.

## Walk-through: the example plugin

`examples/plugins/dice/plugin.toml` takes `prompt`, `tool_result` and `message` and declares
one tool, `roll`. `dice.py` reads one JSON line at a time:

1. `initialize` is answered with `{}`.
2. `message` notifications (no `id`) increment a counter. The plugin keeps state because the
   process lives as long as it is enabled.
3. `prompt` returns one line saying how many messages it has seen, or none before the first.
4. `tool` with `{"tool": "roll", "arguments": {"sides": 6, "count": 2}}` returns text such as
   `4 2 (total 6)`. Invalid arguments raise, which the script reports as an `error` reply.
5. `tool_result` masks e-mail addresses in any tool's output, and returns `{}` when there is
   nothing to change.

To try it:

```sh
mkdir -p ~/.dramatis/plugins
cp -r examples/plugins/dice ~/.dramatis/plugins/dice    # on Windows, set command = ["python", "dice.py"]
```

Open Settings, Plugins, and turn **dice** on. Ask a person to roll two six-sided dice: the
request now carries `dice__roll`, and the answer comes from the script.

The tests in `engine/crates/plugin/tests/hooks.rs` and `engine/crates/agent/tests/plugins.rs`
run this plugin and several deliberately broken ones, without a network.

## Writing a plugin in another language

Any language works that can read lines from stdin and write lines to stdout without
buffering them: flush after each reply. A minimal loop:

1. Read a line, parse the JSON.
2. If it has no `id`, handle the notification and read on.
3. Otherwise compute the result and write `{"id": <same>, "result": ...}` and a newline, or
   `{"id": <same>, "error": {"message": "..."}}`.
4. Exit when stdin closes.
