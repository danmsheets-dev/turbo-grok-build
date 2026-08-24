# Meeting Tool v3 — Teams guest notetaker bot

Spec for `fr_01a030379de47ce1bf74fed2c32cb44b` (must_have, open).
Target release: **1.0.0-rc.9**.

Also closes `fr_01a030361d877ce39a41ff8b933df228` (lobby bot participant).

## Problem

`meeting_join` opens the join URL in the operator's default browser and records
WASAPI loopback locally. Consequences:

- No participant appears in Teams. Operators wait for a lobby admit that never comes.
- Chat Q&A posts through Microsoft Graph **as the signed-in human**, requiring
  `GROK_GRAPH_TOKEN` on that identity.
- Capture dies when the operator leaves the meeting or their machine sleeps.
- Meeting-chat questions run a session turn with the **full** operator toolset.

## Constraints (decided with the operator)

| Constraint | Decision |
|---|---|
| Tenant access | Personal / guest only. No Azure subscription, no admin consent. |
| Hosting | Self-hosted. Meeting audio must not transit a third-party SaaS. |
| Driver | Minimal CDP client written in Rust, driving the already-installed Edge. |
| Trust boundary | Meeting-originated turns get a read-only, workspace-confined toolset. |

Rejected: Recall.ai / MeetingBaaS (audio leaves the machine, per-hour cost);
ACS + Graph calling bot (needs tenant admin); Node + Playwright (ships a Node
runtime and a ~150 MB Chromium); extending the WebView2 host (fights that
crate's allowlist and eval-confirm policy, weak headless story).

## What Teams gives us for free

Teams' default meeting policy is `ExternalBotAccessMode = RequireApprovalWhenDetected`.
Detected external bots are **forced into the lobby regardless of lobby config**,
labeled as bots, and must be admitted individually.

Acceptance criteria 1 and 2 are therefore *inherited*, not built. Our job is to
**observe and report** lobby/admitted state, never to circumvent it.

We do not evade bot detection. If a tenant sets a blocking mode, the bot is
denied and we fall back to local capture with an honest message.

## Architecture

```
  join URL -> MeetingTransport (trait)
                |
       +--------+---------+
       |                  |
  LocalCapture      TeamsGuestBot
  (existing)              |
       |          xai-grok-cdp (new crate)
       |          minimal CDP over tokio WebSocket
       |                  | launches installed Edge --headless=new
       |                  v
       |          teams_tap.js  (injected at document-start)
       |          wraps RTCPeerConnection; taps inbound audio
       |             |                    |
       |    PCM over localhost WS   chat/roster over Runtime.addBinding
       |             |                    |
       +-------------+                    |
                     v                    v
                  pcm_rx            inbox.jsonl        <-- EXISTING SEAMS
                     |                    |
                     v                    v
              run_stt_loop        watch.rs::drain_inbox
                     +---------+----------+
                               v
              extract_turbo_question -> enqueue -> emit_auto_ask
                               v
                        session turn   <-- NEW: read-only ToolKind allowlist
                               v             + ConfinedFs at workspace root
                        meeting_reply
                               v
                bot DOM post -> Graph -> last_reply.md
```

### Seams that do not change

`run_stt_loop` already consumes `mpsc::Receiver<Vec<u8>>`. `watch.rs::drain_inbox`
already drains `inbox.jsonl` alongside Graph. `extract_turbo_question`,
`enqueue_question`, `emit_auto_ask`, `MeetingStore`, and summary composition are
untouched. The bot is a new **producer** for two existing consumers.

### Components

| # | Component | Crate | Responsibility |
|---|---|---|---|
| 1 | `xai-grok-cdp` | new | Generic CDP client. Knows nothing about Teams. |
| 2 | `bot/teams.rs` | `xai-grok-meetings` | Join choreography + selector table. |
| 3 | `bot/teams_tap.js` | `xai-grok-meetings` | In-page audio tap, chat/roster scraping. |
| 4 | `bot/mod.rs` (`MeetingTransport`) | `xai-grok-meetings` | Transport selection and fallback. |
| 5 | `meeting/confine.rs` | `xai-grok-tools` | Read-only toolset for meeting turns. |
| 6 | `meeting/reply.rs` | `xai-grok-tools` | Egress chain: bot -> Graph -> file. |

Selector churn is contained to component 2 plus the selector table. A Teams UI
change is a one-file fix, not a rewrite.

### Audio design

Inbound audio is tapped **inside the page**, not at the OS layer:

1. `Page.addScriptToEvaluateOnNewDocument` installs a wrapper around
   `RTCPeerConnection` before any Teams script runs.
2. The wrapper listens for `track` events, collects inbound audio tracks, and
   routes them through a Web Audio graph into an `AudioWorkletNode`.
3. The worklet downsamples to the STT sample rate, converts to 16-bit PCM, and
   pushes frames over a WebSocket to a loopback server Turbo owns.
4. The server forwards frames into the existing `pcm_tx`.

Consequences: no virtual audio cable, no WASAPI in the bot path, works headless,
independent of operator presence, speakers, or login state. This is what makes
acceptance criterion 7 true.

Audio uses a WebSocket rather than CDP bindings because 20 ms frames are 50
messages/sec and `Runtime.addBinding` base64-inflates. Chat and roster events are
low-rate and do ride CDP bindings.

Outbound audio (Turbo speaking) is designed for but **not implemented**: the
`getUserMedia` override returning a `MediaStreamAudioDestinationNode` is
specified so v2 only needs a TTS source. There is no TTS in `xai-grok-voice`
today. The FR says "and, when possible, spoken audio", so this is in scope as
written.

### Trust boundary

Meeting chat is untrusted input from people who may be outside the org. A
meeting-originated turn gets:

- A **read-only tool allowlist** enforced at dispatch
  (`tool_calls::prepare_tool_call`), keyed off `PromptOrigin::MeetingQuestion`,
  which the pager stamps onto the prompt id via `MEETING_QA_TASK_PREFIX`.
- No shell, no edit, no subagent spawn, and **no MCP / `workspace_tree` /
  `resolve_path`** — MCP reaches off-box and its read-only hints are
  server-self-reported, so it is not trusted with participant input.
- An exemption for the notetaker's own **Q&A subset**
  (`meeting_ask`, `meeting_reply`, `meeting_transcript`, `meeting_status`).
  Every meeting tool is `ToolKind::Meeting`, which is not read-only as a class,
  so without this the turn could not post the answer at all. `meeting_join`,
  `meeting_stop`, `meeting_notes`, `meeting_knowledge` stay blocked.
- The question body carried as data with a `(from <name>)` provenance tag.

Display names in an anonymous-join meeting are spoofable, so we do not gate on
asker identity — the capability restriction is the control.

**Confinement follows the data, not the entry point.** Two paths consume the
same untrusted queue: the automatic answer, and `/meeting ask` with no
arguments. Both are tagged. A `meeting_ask` queue-drain from an untagged turn is
refused outright, so untrusted text cannot be pulled into a full-toolset turn.

A refusal is **non-terminal** (`ToolLoop::PolicyDenied`): it returns as a tool
result so the turn can still answer with what it may use. Cancelling would leave
the coworker with silence and contradict the refusal text.

Filesystem confinement via `ConfinedFs` / `GROK_CONFINE` was considered and not
used: it is process-scoped, and the process is shared with the operator's own
turns, so setting it would confine the operator too.

`meeting_status` currently advertises
`qa: launch workspace + meeting notes (full tools, including MCP)`.
That string becomes accurate for the new posture.

## Data flow

1. `meeting_join(url, title)` parses the URL and selects a transport.
2. `TeamsGuestBot::join` launches Edge headless, navigates to the join URL,
   fills the guest display name, disables camera and microphone, clicks Join.
3. The bot reports `Lobby` while waiting; the operator's Teams client shows
   "Turbo (Notetaker)" awaiting admit. On admit the bot reports `Admitted` and
   the roster becomes readable.
4. `teams_tap.js` streams PCM to the loopback server -> `pcm_tx` -> `run_stt_loop`
   -> transcript + `extract_turbo_question`.
5. Chat messages scraped from the DOM are appended to `inbox.jsonl` ->
   `watch.rs::drain_inbox` -> `extract_turbo_question` -> `emit_auto_ask`.
6. The session turn runs confined and calls `meeting_reply`.
7. `meeting_reply` posts through the bot's DOM composer as the guest identity,
   falling back to Graph, then to `last_reply.md`.

## Failure modes

| Failure | Behavior |
|---|---|
| Edge not found | Fall back to `LocalCapture`; join output states why. |
| Bot never admitted (timeout) | Stay in lobby, keep reporting; operator may stop. Configurable timeout, default 5 min, then fall back to `LocalCapture`. |
| Bot denied / removed | Fall back to `LocalCapture`; status reports `BotDenied`. |
| Meeting requires sign-in | Detect the signed-in-only join page, fall back immediately with a clear message. |
| CAPTCHA presented | **Never solved.** Report and fall back. Microsoft is retiring join CAPTCHA, but we do not depend on that. |
| STT falls behind | Frames are **shed, not queued** — a `bufferedAmount` ceiling in the page and `try_send` in Turbo. Buffering through a stall only makes stale audio. `meeting_status` reports `notetaker_audio_dropped`. |
| Audio socket closes mid-meeting | The page reopens it (throttled to 2 s) rather than going silently mute. |
| CDP connection drops | Bot marked dead, `LiveMeeting` shuts down cleanly, transcript preserved. |
| Graph post fails | Already handled; bot path is tried first, `last_reply.md` always written. |
| Selector not found | Named `SelectorError` identifying which step failed, so UI churn is diagnosable from logs. |

Capture and recap must survive every one of these — acceptance criterion 6.

## Testing

- **`xai-grok-cdp`**: unit tests for framing, request/response correlation,
  binding dispatch, and target lifecycle against a mock WebSocket peer.
- **Transport**: `MeetingTransport` has a `MockTransport` so join/lobby/admit/
  denied/fallback state transitions are tested without a browser.
- **Selector table**: parsed and asserted non-empty at compile time; a golden
  test pins the documented step names.
- **Audio**: the PCM path is tested by driving the loopback server directly with
  synthetic frames and asserting they reach `pcm_rx`.
- **Confinement**: assert the meeting toolset excludes write/shell/spawn kinds
  and that the FS backend is confined — a real restricted toolset, not zero tools.
- **Chat ingress**: append to `inbox.jsonl`, assert `emit_auto_ask` fires.
- Live Teams join is **manual** and gated behind `GROK_MEETING_BOT=1`; CI never
  joins a real meeting.

## Out of scope

- Spoken answers (no TTS). Designed for, not built.
- Zoom / Meet / Webex bots. The trait admits them later; v3 is Teams-only.
- Signed-in bot identity (needs a second M365 account).

## Already shipped, to be marked in the FRL

Verified by passing tests, not by inspection:

- `fr_01a02f4c398c7bf290c44c19a099873e` — `detect_join_operator_meeting_link_and_qa_phrasing`
- `fr_01a02f4c39937783b52b29c220af2910` — `vocative_comma_and_space`
- `fr_01a02f50986078b3a3d855bc63cca9a9` — `redact_join_secrets_strips_passcode_query` + `graph:` in `format_meta`

`fr_01a0244ce3a97b3295bab30d6871cb64` should be amended: its "no WebView bot"
guidance assumed bot detection blocks bots. Detection defaults to lobby +
label + approve, which is the desired UX.
