# Meeting Notetaker (v3)

Turbo's Fathom-style meeting notetaker. From **1.0.0-rc.9** it joins Teams
meetings as a **visible guest participant** instead of recording this machine.

| Version | Transport | What other participants see |
|---------|-----------|-----------------------------|
| v1 (rc.4) | WASAPI loopback on the operator's PC | nothing |
| v2 (rc.8) | same, plus Graph chat Q&A as the operator | nothing |
| **v3 (rc.9)** | **joined guest bot** | **"Turbo (Notetaker)" in the lobby, then in Participants** |

## How a join goes

```bash
turbo
# then, in the pager:
/meeting join https://teams.microsoft.com/l/meetup-join/...
```

1. Turbo launches the **Edge already installed on the machine**, headless, with
   a throwaway profile under the meeting folder.
2. It fills the guest name **Turbo (Notetaker)**, turns the camera and
   microphone off, and clicks Join.
3. Teams puts it in the **lobby**. `meeting_status` reports
   `waiting in the lobby — admit "Turbo" to start notes`.
4. You admit it. It appears in Participants, distinct from you.
5. Audio, chat, and roster start flowing. `Turbo:` questions get answered.

Nothing is recorded before admission, because nothing is heard before admission.

### Teams holds bots in the lobby on purpose

Teams' default meeting policy is `ExternalBotAccessMode = RequireApprovalWhenDetected`:
detected external notetakers are placed in the lobby **regardless of your lobby
settings**, labelled as bots, and must be admitted individually.

Turbo does not work around this. It joins as an ordinary anonymous guest under a
self-identifying name and reports whatever Teams decides. There is no attempt to
evade bot detection, and **verification challenges are never answered** — a
challenge ends the bot join and falls back to local capture.

If a tenant blocks external bots outright, the join fails cleanly and says so.

## Audio

Audio is tapped **inside the meeting page**, not from your sound card:

- A document-start script wraps `RTCPeerConnection` and collects inbound audio.
- Web Audio runs natively at 16 kHz, so no resampling is needed.
- An `AudioWorklet` emits 20 ms frames of 16 kHz mono 16-bit LE PCM.
- Frames go over a loopback WebSocket, bound to `127.0.0.1` with a random
  per-meeting token, into the same Grok STT pipeline v1 used.

If STT falls behind (an auth retry or socket reconnect), frames are **dropped
rather than queued** — both in the page and in Turbo — because audio buffered
through a stall is stale by the time it is transcribed. `meeting_status` reports
`notetaker_audio_dropped` so a gappy transcript is visible rather than silent.

Consequences:

- **Your speakers, headset, and microphone are irrelevant.** Nothing on the
  machine is recorded.
- **You can leave the meeting** and the notetaker keeps listening.
- The bot's own outbound audio track is **silent by construction** — it is a
  zero-gain Web Audio node, not Chromium's fake-device beep.

Turbo cannot speak in the meeting yet. That needs TTS, which does not exist in
`xai-grok-voice` today; the injection point is reserved.

## Chat Q&A and the trust boundary

Anyone in the meeting can type `Turbo: how is the website going` — including
external guests, whose display names are spoofable.

**Meeting-driven turns are confined to read-only tools.** This is enforced at
tool dispatch, not merely requested in the prompt: the pager tags the prompt id
with `meeting-qa-`, the shell parses that into `PromptOrigin::MeetingQuestion`,
and anything outside the allowed set is refused before it runs.

Allowed in a meeting turn:

- `read_file`, `grep`, `list_dir`, LSP, memory lookup, web search, web fetch
- `meeting_ask`, `meeting_reply`, `meeting_transcript`, `meeting_status`

Refused — including for the notetaker's own tools:

- writes, edits, shell, subagent spawn
- `meeting_join`, `meeting_stop`, `meeting_notes`, `meeting_knowledge`, so a
  coworker cannot start another recording, end this one, rewrite the recap, or
  repoint the knowledge folder
- **MCP servers**, `workspace_tree`, and `resolve_path` — MCP tools reach
  off-box and their read-only hints are self-reported by the server, so they
  are not trusted with participant-authored input

So a coworker can get an answer about your work. A coworker cannot make Turbo
edit a file, run a shell command, or spawn a subagent — and if the classification
is ever unreadable, it **fails closed**.

Two paths consume the same untrusted queue and **both** are confined: the
automatic answer when someone asks, and `/meeting ask` with no arguments, which
drains a queued participant question. Calling `meeting_ask` with no question
from an ordinary turn is refused outright, so the confinement travels with the
data rather than with the entry point. `/meeting ask <your own question>` is
your words, and runs as a normal turn.

A refusal does not kill the turn: it comes back as a tool result so Turbo can
still answer with what it *is* allowed to use.

Answers post to meeting chat as **Turbo (Notetaker)**. If the bot is not in the
meeting, Turbo falls back to Microsoft Graph (posting as you, when
`GROK_GRAPH_TOKEN` is set), and always writes `last_reply.md`.

## When the bot cannot join

Every failure falls back to the old local-capture path and **says so**, naming
what is actually being recorded:

| Situation | Result |
|---|---|
| No Edge or Chrome installed | local capture |
| Not a Teams URL (Zoom, Meet, Webex) | local capture |
| `GROK_MEETING_BOT=0` | local capture |
| Meeting requires signed-in users | local capture |
| Verification challenge shown | local capture (never solved) |
| Teams UI changed, a step not found | local capture, naming the step |
| Teams served the desktop-app launcher | local capture, named `Teams app launcher` |

Sitting in the lobby past the timeout marks the notetaker `Failed` and reports
it. Turbo does **not** silently switch to recording your speakers instead —
starting a different kind of capture without saying so would be a surprise.

A failed guest join **leads** with what did not happen:

```
NO GUEST IN THE MEETING - the notetaker could not join (Teams app launcher).
Nobody is in the lobby and chat Q&A through the notetaker is unavailable.
Local recording started (Teams)
```

The outcome is durable in the meeting's `meta.json`, so `meeting_status` and
`meeting_stop` report the same thing after a restart. A transcript from local
capture looks healthy either way, which is exactly why the difference has to be
stated rather than inferred.

## The desktop-app launcher hop

A Teams `/meet/<id>` link redirects to
`/dl/launcher/launcher.html?…&msLaunch=true&directDl=true&suppressPrompt=true`,
which fires the `ms-teams:` protocol immediately and never renders
"Continue on this browser" — so the guest notetaker has no join screen to drive,
and any handoff would reach your *signed-in desktop client*, not an anonymous
guest.

Turbo defends this in four layers, deliberately, because two of them rest on
Teams behaviour that is observed rather than documented:

1. **Navigation logging.** Every hop is logged (`notetaker navigation`), query
   strings stripped. This is how you tell whether the rest is working.
2. **Page-side guard.** The injected tap refuses `ms-teams:` / `msteams:` /
   `teams:` navigations and retries the continue-on-web click from its own poll
   loop, because the launcher redirects twice inside a second while the Rust
   side polls at 500 ms.
3. **URL rewrite.** The bot navigates a query-only rewrite asking for the
   anonymous web client. Host, path and the `p` passcode are untouched;
   unrecognised link shapes are navigated exactly as pasted. Disable with
   `GROK_MEETING_TEAMS_WEB=0`.
4. **Downloads denied browser-wide,** so `directDl=true` cannot pull an
   installer.

Layers 3 and 4 are the unverified ones. If a join still fails, run with
`GROK_MEETING_BOT_WINDOW=1` and check whether `/dl/launcher/` still appears in
the navigation log.

Note that `meeting_join` does **not** hand the link to your OS when a guest bot
is dispatched — the notetaker opens it itself, in an isolated profile. Only the
local-capture paths open it for you, because only those need you in the meeting.

## Environment

| Variable | Default | Meaning |
|---|---|---|
| `GROK_MEETING_BOT` | on | `0`/`false`/`off` forces local capture |
| `GROK_MEETING_BOT_WINDOW` | off | `1` shows the browser window (diagnosing a failed join) |
| `GROK_MEETING_LOBBY_TIMEOUT` | `300` | Seconds to wait for an admit |
| `GROK_MEETING_SELECTORS` | unset | Path to a selector-override JSON file |
| `GROK_CDP_BROWSER` | unset | Absolute path to a Chromium binary |
| `GROK_MEETING_TEAMS_WEB` | on | `0` navigates the pasted URL as-is instead of asking Teams for the web client |
| `GROK_MEETING_NO_CAPTURE` | off | Disable audio entirely (tests) |
| `GROK_MEETING_CAPTURE` | auto | `mic` / `loopback` for the fallback path |
| `GROK_MEETING_AUTO_ASK` | on | `0` queues `Turbo:` questions instead of answering; drain them with `/meeting ask` |
| `GROK_GRAPH_TOKEN` | unset | Delegated Graph token for the chat fallback |

## When Teams changes its UI

Microsoft ships UI changes on its own schedule. Every selector is a candidate
list in one file, and the whole table can be replaced from disk without waiting
for a Turbo release:

```bash
# Dump the current defaults, edit, then point Turbo at the result.
export GROK_MEETING_SELECTORS=~/.grok/teams-selectors.json
```

A file at `$GROK_HOME/teams-selectors.json` is picked up automatically. Partial
overrides are fine — unspecified groups keep their defaults. A malformed file is
reported rather than silently ignored, so a typo is not misread as "Teams
changed again".

Join failures name the step that could not be found (`name_input`,
`join_button`, `chat_input`, …), which is the key to edit.

## Files

Per meeting, under the session folder:

```
meetings/<id>/
  meta.json          status, platform, capture source, title
  transcript.jsonl   STT segments
  inbox.jsonl        scraped chat — the shared ingress seam
  questions.jsonl    pending Turbo: questions
  notes.md           session notes
  last_reply.md      most recent [Turbo] answer
  bot-profile/       throwaway browser profile
```

The work-only recap lands in `{workspace}/Meetings/YYYY-MM-DD - <name>.md`.

## Architecture

Two crates, both Teams-agnostic at the bottom:

- **`xai-grok-cdp`** — minimal Chrome DevTools Protocol client (Target, Page,
  Runtime) over a local WebSocket. Launches the installed browser through
  `ProcessScope::enroll`, so the whole Chromium tree is reaped with the session
  and a notetaker can never outlive the meeting it was recording.
- **`xai-grok-meeting-bot`** — join choreography, selector table, injected tap,
  loopback audio server.

The bot is a new *producer* for two seams that already existed: `pcm_rx` feeding
`run_stt_loop`, and `inbox.jsonl` feeding `watch::drain_inbox`. Transcript
storage, `Turbo:` detection, auto-ask, and recap composition are unchanged.
