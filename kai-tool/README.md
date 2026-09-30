# kai-tool

Kai launches Codex and resumes conversations with configurable credential rotation.

## Usage

```text
kai                         Launch Codex
kai resume                  Open the all-sessions picker
kai resume ID               Resume a specific conversation
kai status                  List local Kai windows and their latest turn state (Linux)
kai status --watch          Refresh the window dashboard every two seconds
kai llm-get PATH...          Assemble a source listing
```

Unambiguous command prefixes work, such as `kai r` and `kai llm`.

`--fast` selects the Fast service tier. Custom builds with a `-k.` prerelease suffix followed
by exactly eight hexadecimal commit characters (for example, `0.154.0-k.ac192cd7`) are supervised
automatically. `--no-auto-restart` disables supervision. Kai runs Codex with its approval/sandbox bypass flag.

Every launch and automatic recovery passes Codex config overrides for 16 concurrent spawned agents,
the `monokai-extended` theme, disabled decorative whimsy, colored status indicators, the saved
session's working directory on resume, and hidden rate-limit model-switch reminders. The status
line shows model/reasoning, run
state, remaining context, weekly limit, total input/output tokens, and Fast-mode status, in that order.

Install with `cargo install --path kai-tool --locked --force`.

## Window status

`kai status --watch` shows the current user's local Kai launcher processes, including windows
opened before the dashboard started. Each row includes the PID, terminal, working directory,
conversation ID, and the age of the latest recorded turn completion. Use `--interval SECONDS`
to change the refresh rate (1–3600 seconds), and press `q`, Esc, or Ctrl-C to quit.

| State | Meaning |
| --- | --- |
| Working | The main conversation has a recorded turn start without a matching end. |
| Ready | The latest recorded turn completed successfully; the window remains open. |
| Needs input | A synchronous `request_user_input` call is awaiting its matching result. |
| Interrupted | The latest recorded turn was aborted. |
| Error | The latest recorded turn completion contains an error. |
| Unknown | There is insufficient evidence to identify the main conversation or its state. |
| Exited | The dashboard observed the window process exit. Its exit result is unavailable. |

“Last finished” refers to a turn ending, including interruptions and errors; it does not mean
the overall task was accomplished. Exited windows stay visible for ten minutes while the watcher
runs. Windows closed before observation are not included, and history is not saved between runs.

This first implementation requires Linux `/proc`. It reads same-user Kai/Codex process metadata
and the session logs held open by a direct Codex child. It does not require a Codex binary on PATH,
call a credential provider, start a daemon, or make model/API requests. It works with the custom
Codex build's embedded server; detached servers, remote machines, and processes hidden by `/proc`
permissions may be unavailable. Subagent logs are excluded. Multiple open main conversations in
one process are reported as Unknown because the log does not identify the visible conversation.

State follows persisted Codex `task_started`/`task_complete`/`turn_aborted` events (also accepting
`turn_started`/`turn_complete`), so updates can lag buffered log writes. It does not detect every
approval prompt or infer progress from CPU use. Async questions do not imply the agent is blocked.
Unknown is also used during startup/recovery or when logs are missing, unreadable, malformed, or
from an unsupported format. Reads are incremental and bounded to the most recent 16 MiB on startup
or when catching up, skipping individual records over 1 MiB. A skipped record makes state Unknown
until a later lifecycle event provides evidence. Prompt text, tool arguments, responses, and error
messages are not included in output. LLM progress summaries are a separate feature.

For scripts, `kai status --json` emits one snapshot; `kai status --watch --json` emits one JSON
object per line, including when output is piped. Snapshot `version` is `1`, `observed_at` is Unix
seconds, and `windows` contains `pid`, `tty`, `cwd`, `thread_id`, `state`, `last_finished_at`,
`exited_at`, and `detail`. Unavailable values are null, timestamps are Unix seconds, and state names
are `working`, `ready`, `needs_input`, `interrupted`, `error`, `unknown`, or `exited`. `warnings`
reports incomplete process discovery. No terminal escape sequences are emitted in JSON mode.

## Credential provider

Set `${XDG_CONFIG_HOME:-~/.config}/kai/config.toml`:

```toml
credential_provider = "/path/to/provider.sh"
```

`--credential-provider SCRIPT` overrides that setting. Configuration-relative paths resolve
from the configuration directory; CLI paths resolve from the current directory.

The script has two operations:

```text
bash SCRIPT acquire --codex-home PATH --sqlite-home PATH

bash SCRIPT next --codex-home PATH --sqlite-home PATH \
  --auth-file PATH --credential-use-lock PATH \
  --credential-use-lock-mode shared|exclusive \
  --credential-mutation-lock PATH --available-file PATH \
  --cause quota-exhausted|credential-invalid|model-capacity [--unavailable-until UNIX_SECONDS]
```

Each returns exactly one JSON object:

```json
{
  "auth_file": "/pool/credential/auth.json",
  "credential_use_lock": "/pool/credential/use.lock",
  "credential_use_lock_mode": "shared",
  "credential_mutation_lock": "/pool/credential/mutation.lock",
  "available_file": "/pool/credential/available"
}
```

The provider chooses the credential and its shared/exclusive use-lock mode. Kai locks the returned
file accordingly, verifies availability, and transfers the held descriptor to Codex. Consumers
retain that lock while using the credential and serialize token changes with the mutation lock,
reloading the shared auth file after locking it.

Paths must be distinct and absolute, with private regular files owned by the user and mode 0600.
The provider owns the files. Removing the availability marker prevents new selections. The two
home arguments are opaque launch context.

Quota exhaustion, permanent credential failure, or model capacity invokes `next`. Kai forwards the reported
reset timestamp, then restores Codex's input handoff and resumes the conversation. Normal exit and
crashes invoke no hook; the operating system releases held locks.

Script stdin is closed, stdout carries the JSON response, and stderr remains visible. Ambient
Codex/OpenAI auth variables are removed. Responses are limited to 64 KiB; selection has a
30-second timeout. Managed Codex uses credential protocol version 2 and transfers its descriptor
through `SCM_RIGHTS` with the private, nonce-authenticated READY/GO startup handshake.

Hooks run only during supervision of these custom builds. Otherwise Codex uses ordinary authentication.
With supervision enabled and no provider, Kai aborts if credential rotation becomes necessary.

## Source listings

`kai llm-get` recursively selects source files, prepends local `AGENTS.md` and `DESIGN.md`,
and copies the listing to the clipboard. Use `--out -` for stdout, `--out PATH` for a file,
or `--slim` to omit instruction files. Run `kai llm-get --help` for filtering options.

## License

AGPL-3.0-only
