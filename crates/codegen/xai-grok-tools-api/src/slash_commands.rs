//! Canonical slash-command wording (`/loop`, `/schedule`, `/imagine`, `/imagine-video`, `/goal`),
//! shared by every front-end (Grok Build shell/pager and other hosts) so
//! expansions cannot drift.

/// Canonical tool name advertised by the scheduler create tool. Gating code
/// (shell `CommandAvailability`, pager `required_tools`, host command lists)
/// keys `/loop` availability on this name.
pub const SCHEDULER_CREATE_TOOL_NAME: &str = "scheduler_create";

/// Usage hint shown when `/loop` is invoked with no arguments.
pub fn loop_usage_message() -> &'static str {
    "Usage: /loop [interval] <prompt>\n\
     Example: /loop 30m check deploy status\n\
     Example: /loop check deploy status every hour\n\n\
     Tell me how often it should run (e.g. 30m, 1 hour, every 2 days)."
}

/// Where a scheduled fire runs, which decides what the stored prompt can rely on.
///
/// Resolved from `[scheduler] background_loops` (env, config, managed policy and
/// remote settings all feed it), so `/loop` describes the runtime the user
/// actually has rather than hedging across both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopFireMode {
    /// Each fire runs in a detached background subagent that cannot see this
    /// conversation. The default.
    Detached,
    /// Each fire runs as a turn in this conversation, where earlier results from
    /// the same task may still be visible.
    InSession,
}

/// Build the model instruction that `/loop` expands into for `args`.
///
/// The model, not brittle host parsing, turns the request into the
/// `scheduler_create` interval, accepting every natural phrasing and erroring
/// on bad input rather than silently defaulting. See [`loop_usage_message`].
///
/// Only the framing differs by `mode`; the stop condition and length guidance
/// are identical, because both hold wherever the fire runs.
pub fn loop_schedule_instruction(args: &str, mode: LoopFireMode) -> String {
    let fire_context = match mode {
        LoopFireMode::Detached => {
            "Each fire runs in a detached background subagent, not in this conversation,\n\
             so the prompt you store must stand on its own.\n\n\
             ## Writing a prompt that survives a fresh fire\n\
             - Inline the state a fire needs: paths, job/PR/branch ids, the command that checks\n\
               status, and what \"healthy\" looks like. A fire cannot see this conversation, and\n\
               a long-running task restarts from a short summary every few iterations.\n\
             - Only a short status comes back here, so say what that status must contain."
        }
        LoopFireMode::InSession => {
            "Each fire arrives as a new turn in this conversation, and earlier results from\n\
             the same task may still be above it. The stored prompt is re-sent verbatim every\n\
             time, so write a standing order rather than a one-off request.\n\n\
             ## Writing a prompt that reads well on every fire\n\
             - Name the state that must not be guessed: paths, job/PR/branch ids, the command\n\
               that checks status, and what \"healthy\" looks like. This conversation is\n\
               compacted as it grows, so do not rely on details staying visible.\n\
             - Earlier fires may be above you: continue from them instead of restarting."
        }
    };
    format!(
        "# /loop -- schedule a recurring prompt\n\n\
         Turn the input below into a scheduler_create call. {fire_context}\n\
         - Say what one fire does and when it bails: \"if still pending, report one line and\n\
           stop.\" A fire must not poll inline.\n\
         - Give it a stop condition and an exit: \"when <condition> holds, report it and call\n\
           scheduler_delete <task_id>.\" Without that the loop runs until it expires.\n\
         - Keep it short and concrete -- the stored prompt is re-sent on every fire.\n\n\
         ## Deriving the interval\n\
         Convert the user's cadence -- however phrased, at either end of the request -- into a\n\
         compact `<number><unit>` string (`s`/`m`/`h`/`d`); the remaining text is the prompt.\n\
         The minimum is 60 seconds and shorter values are raised, so say so when it applies.\n\
         If no cadence is given, ask the user how often it should run -- never invent one.\n\n\
         ## Action\n\
         Schedule from what the user already gave you \u{2014} do not explore the workspace or run\n\
         checks before scheduling; the first fire does that.\n\
         1. Call scheduler_create with the interval, the prompt, and fire_immediately: true.\n\
            If the interval is rejected, fix the string rather than guessing.\n\
         2. Confirm what's scheduled, the cadence, its stop condition, that it auto-expires\n\
            after 7 days, and the task_id to cancel with scheduler_delete.\n\
         3. Do NOT execute the prompt inline. The scheduler fires it immediately.\n\n\
         ## Wrong tool for the job\n\
         - \"Tell me when X finishes\" -> a background command or watch tool that wakes you on\n\
           the event, not a recurring loop that re-checks on a timer.\n\
         - \"Do X once in N minutes\" -> background `sleep <secs> && <command>`; scheduling is\n\
           recurring-only.\n\n\
         ## Changing an existing loop\n\
         Call scheduler_create with its task_id and only the changed fields; do not\n\
         delete and recreate. If later work changes what a loop should do, update its\n\
         prompt the same way.\n\n\
         ## Input\n\
         {args}"
    )
}

/// Canonical name of the image generation tool; gates `/imagine`.
pub const IMAGE_GEN_TOOL_NAME: &str = "image_gen";

/// Advertised name of the /imagine command.
pub const IMAGINE_COMMAND_NAME: &str = "imagine";

/// Canonical name of the image-to-video tool; gates `/imagine-video`.
pub const IMAGE_TO_VIDEO_TOOL_NAME: &str = "image_to_video";

/// Advertised name of the /imagine-video command.
pub const IMAGINE_VIDEO_COMMAND_NAME: &str = "imagine-video";

/// Usage hint shown when `/imagine` is invoked with no arguments.
pub fn imagine_usage_message() -> &'static str {
    "Usage: /imagine <description>\n\
     Provide a text description to generate an image."
}

/// Build the model instruction that `/imagine` expands into for `prompt`.
pub fn imagine_instruction(prompt: &str) -> String {
    format!(
        "Call the image_gen tool immediately, passing the user's prompt below \
         verbatim — do not rewrite, embellish, or expand it. \
         After the tool completes, briefly acknowledge and mention \
         where the image was saved.\n\n\
         Prompt: {prompt}"
    )
}

/// Usage hint shown when `/imagine-video` is invoked with no arguments.
pub fn imagine_video_usage_message() -> &'static str {
    "Usage: /imagine-video <description>\n\
     Provide a text description to generate a video."
}

/// Build the model instruction that `/imagine-video` expands into for `prompt`.
pub fn imagine_video_instruction(prompt: &str) -> String {
    format!(
        "{IMAGINE_VIDEO_SKILL}\n\n\
         User prompt: {prompt}"
    )
}

/// Video workflow guidance injected by `/imagine-video`.
const IMAGINE_VIDEO_SKILL: &str = "\
# Imagine Video

Video starts from an image — there is no text-to-video tool. \
Default to `image_to_video`; use `reference_to_video` only when the user \
explicitly asks for it or a shot genuinely needs multiple reference images.

## Default: single clip

Unless the user asks for a long video, multiple scenes, or a multi-shot sequence, \
generate **one** video:

1. Create a source image with `image_gen` that stages the first frame \
(composition, subject, lighting).
2. Call `image_to_video` with that image and a short prompt describing the motion \
or camera move (1–2 sentences, present tense).
3. After the tool completes, mention the saved file path so the user can find it.

## Longer / multi-shot videos

When the user requests a longer video, multiple scenes, or a narrative sequence:

1. **Plan the story as shots** — break the idea into distinct shots, one beat each.
2. **Favor frequent, short shots** — prefer more 6s clips over fewer long ones; more cuts keep it dynamic.
3. **Create each shot's source image** with `image_gen` (or `image_edit` to combine references), keeping characters and settings consistent across shots.
4. **Animate each shot with `image_to_video`** — the source image becomes frame 1.
5. **Assemble with FFmpeg** using stream copy (`ffmpeg -f concat ... -c copy` — never re-encode). \
Keep every shot at the same resolution and frame rate so the concat works. \
After assembly, mention the final output path.

## Shot guidance

- **Prompt-craft:** one short, vivid moment in present tense with a clear camera movement, in 1–2 sentences.
- **Minimal but interesting:** one clear subject, one simple motion or camera move per shot. Avoid complex multi-action animation; make the shot compelling through composition, lighting, and a strong moment.
- **Complex source image?** Intricate frames (busy geometry, fine detail, heavy reflections) warp when animated. Keep the subject fixed and move only the camera (slow push-in, orbit, or parallax), or break into simpler shots. For new shots, generate a simpler, animation-friendly base image rather than animating a busy one.
- **`image_to_video` animates from frame 1** — stage the first frame with `image_gen`/`image_edit` before animating.
- **Aspect ratio:** set it on the source image (`image_gen` `aspect_ratio`); don't re-crop an existing video.
- **Duration:** 6s or 10s only (prefer 6s); round to the nearest.
- **Real people:** reference-first — drive the video from a verified reference image; never animate a named person without one.
- Don't loop the same clip unless asked.";

/// Canonical tool name advertised by the scheduler list tool. Gates `/schedule list`.
pub const SCHEDULER_LIST_TOOL_NAME: &str = "scheduler_list";

/// Canonical tool name advertised by the scheduler delete tool. Gates `/schedule cancel`.
pub const SCHEDULER_DELETE_TOOL_NAME: &str = "scheduler_delete";

/// Advertised name of the /schedule command.
pub const SCHEDULE_COMMAND_NAME: &str = "schedule";

/// Usage hint shown when `/schedule` is invoked with no arguments.
pub fn schedule_usage_message() -> &'static str {
    "Usage: /schedule [at|every] <when> <prompt-or-recipe>\n\
     /schedule list\n\
     /schedule show <id>\n\
     /schedule cancel <id>\n\n\
     When: 5m, 1h, 1d, at 2026-08-24T09:00, every weekday 08:00\n\
     Recipes: search <query> | stat <url-or-query> | meeting join <url> [name]\n\n\
     Standing jobs do not auto-expire (unlike /loop's 7-day cap). \
     Results go to Schedules/YYYY-MM-DD - <title>.md."
}

/// `/schedule` verb after the slash command name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleVerb<'a> {
    List,
    Show { id: &'a str },
    Cancel { id: &'a str },
    Create { rest: &'a str },
}

/// Split `/schedule` args into list/show/cancel vs create.
pub fn parse_schedule_verb(args: &str) -> ScheduleVerb<'_> {
    let trimmed = args.trim();
    let (first, rest) = trimmed
        .split_once(char::is_whitespace)
        .map(|(a, b)| (a, b.trim()))
        .unwrap_or((trimmed, ""));
    match first.to_ascii_lowercase().as_str() {
        "list" | "ls" => ScheduleVerb::List,
        "show" if !rest.is_empty() => ScheduleVerb::Show { id: rest },
        "cancel" | "delete" | "rm" if !rest.is_empty() => ScheduleVerb::Cancel { id: rest },
        _ => ScheduleVerb::Create { rest: trimmed },
    }
}

/// Recipe parsed from the prompt body (after the when-clause).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleRecipe<'a> {
    Search { query: &'a str },
    Stat { target: &'a str },
    MeetingJoin { url: &'a str, name: Option<&'a str> },
    Freeform { prompt: &'a str },
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// Parse a recipe token at the start of `body`.
pub fn parse_schedule_recipe(body: &str) -> ScheduleRecipe<'_> {
    let body = body.trim();
    if let Some(query) = strip_prefix_ci(body, "search ") {
        let query = query.trim();
        if !query.is_empty() {
            return ScheduleRecipe::Search { query };
        }
    }
    if let Some(target) = strip_prefix_ci(body, "stat ") {
        let target = target.trim();
        if !target.is_empty() {
            return ScheduleRecipe::Stat { target };
        }
    }
    if let Some(rest) = strip_prefix_ci(body, "meeting join ") {
        let rest = rest.trim();
        if !rest.is_empty() {
            let (url, name) = rest
                .split_once(char::is_whitespace)
                .map(|(u, n)| (u, Some(n.trim())))
                .unwrap_or((rest, None));
            return ScheduleRecipe::MeetingJoin { url, name };
        }
    }
    ScheduleRecipe::Freeform { prompt: body }
}

fn recipe_title(recipe: &ScheduleRecipe<'_>) -> String {
    match recipe {
        ScheduleRecipe::Search { query } => truncate_title(query),
        ScheduleRecipe::Stat { target } => truncate_title(target),
        ScheduleRecipe::MeetingJoin { name, url } => name
            .filter(|n| !n.is_empty())
            .map(|n| truncate_title(n))
            .unwrap_or_else(|| truncate_title(url)),
        ScheduleRecipe::Freeform { prompt } => truncate_title(prompt.lines().next().unwrap_or(prompt)),
    }
}

fn truncate_title(s: &str) -> String {
    let t = s.trim();
    if t.chars().count() <= 60 {
        t.to_string()
    } else {
        t.chars().take(60).collect()
    }
}

fn schedules_write_blurb(title: &str) -> String {
    format!(
        "Write the result to `Schedules/YYYY-MM-DD - {title}.md` in the launch workspace \
         (today's local date). Do not use `..` in the path. Do not write if `Schedules` \
         is a symlink. Prefer the write tool."
    )
}

/// Expand a recipe into the stored fire prompt, title, and meeting-join flag.
pub fn expand_schedule_recipe(body: &str) -> (String, String, bool) {
    let recipe = parse_schedule_recipe(body);
    let title = recipe_title(&recipe);
    let meeting_join = matches!(recipe, ScheduleRecipe::MeetingJoin { .. });
    let prompt = match recipe {
        ScheduleRecipe::Search { query } => format!(
            "Search the web for: {query}\n\n\
             Produce a concise briefing: what you found, notable sources, and what is new. \
             Do not invent citations.\n\n\
             {}",
            schedules_write_blurb(&title)
        ),
        ScheduleRecipe::Stat { target } => format!(
            "Fetch and summarize current status/metrics for: {target}\n\n\
             Produce a metric snapshot: headline numbers, status, timestamp, and source URL if any.\n\n\
             {}",
            schedules_write_blurb(&title)
        ),
        ScheduleRecipe::MeetingJoin { url, name } => {
            let title_line = name
                .filter(|n| !n.is_empty())
                .map(|n| format!(" title: {n}"))
                .unwrap_or_default();
            format!(
                "Call meeting_join with url: {url}{title_line}.\n\
                 Do NOT use bash, Start-Process, or open the URL yourself — only meeting_join.\n\
                 After joining, confirm the notetaker is running."
            )
        }
        ScheduleRecipe::Freeform { prompt } => {
            format!("{prompt}\n\n{}", schedules_write_blurb(&title))
        }
    };
    (prompt, title, meeting_join)
}

/// Build the model instruction that `/schedule` expands into.
pub fn schedule_instruction(args: &str) -> String {
    match parse_schedule_verb(args) {
        ScheduleVerb::List => {
            "# /schedule list\n\n\
             Call scheduler_list now. Summarize each task: id, title or prompt, cadence, \
             next fire, and whether it expires. Do not create or cancel anything."
                .into()
        }
        ScheduleVerb::Show { id } => format!(
            "# /schedule show {id}\n\n\
             Call scheduler_list and show the task whose id is {id} in full \
             (prompt, cadence, next fire, expiry). If it is missing, say so."
        ),
        ScheduleVerb::Cancel { id } => format!(
            "# /schedule cancel {id}\n\n\
             Call scheduler_delete with id: {id}. Confirm cancellation. \
             If it is missing, say so and suggest scheduler_list."
        ),
        ScheduleVerb::Create { rest } => {
            let (expanded, title, meeting_join) = expand_schedule_recipe(strip_leading_when(rest));
            format!(
                "# /schedule -- standing product job\n\n\
                 Turn the input below into a scheduler_create call for a **standing** job.\n\n\
                 ## Defaults (do not change unless the user asked)\n\
                 - durable: true\n\
                 - standing: true  (no 7-day expiry — unlike /loop)\n\
                 - fire_immediately: false  (wait for the first interval or `at`)\n\
                 - Each fire runs as a **background subagent**, isolation=worktree, \
                   capability read-only unless this is a meeting-join recipe.\n\
                 - Overlap skip is already handled by the scheduler (in-flight last_subagent_id).\n\n\
                 ## When\n\
                 - Interval: compact `<number><unit>` (`5m`, `1h`, `1d`, `60s`; min 60s).\n\
                 - One-shot: pass `at` as ISO-8601 / `2026-08-24T09:00` (local if no timezone). \
                   Do not pass `interval`. The tool sets interval = seconds until then (min 60s) \
                   and recurring=false. The time MUST be in the future.\n\
                 - Weekday clock: pass `at` as `weekday 08:00` or `monday 09:00` (SHOULD).\n\
                 - If no when is given, ask — never invent a cadence.\n\n\
                 ## Recipes (expand the stored prompt; do not store the raw recipe token alone)\n\
                 - `search <query>` — web briefing; must write `Schedules/YYYY-MM-DD - <title>.md`.\n\
                 - `stat <url-or-query>` — metric snapshot; must write the same Schedules path.\n\
                 - `meeting join <url> [name]` — stored prompt MUST call `meeting_join` \
                   (not Start-Process / bash). Set meeting_join: true.\n\
                 Confirm-once for meeting/write is a prompt concern; do not open URLs yourself.\n\n\
                 ## Suggested stored prompt\n\
                 Title: {title}\n\
                 meeting_join: {meeting_join}\n\
                 Prompt:\n\
                 {expanded}\n\n\
                 ## Action\n\
                 1. Call scheduler_create with standing: true, durable: true, fire_immediately: false, \
                    title, the expanded prompt, and interval and/or at as derived. \
                    Set meeting_join: true only for the meeting-join recipe.\n\
                 2. Confirm what's scheduled, the cadence, that it does **not** auto-expire, \
                    that results go under Schedules/, and the task_id to cancel with \
                    `/schedule cancel` or scheduler_delete.\n\
                 3. Do NOT execute the prompt inline. The scheduler fires it later.\n\n\
                 ## Input\n\
                 {rest}"
            )
        }
    }
}

/// Best-effort strip of a leading when-clause so recipe expansion can see `search`/`stat`.
fn strip_leading_when(rest: &str) -> &str {
    let t = rest.trim();
    let (first, after) = t
        .split_once(char::is_whitespace)
        .map(|(a, b)| (a, b.trim()))
        .unwrap_or((t, ""));
    let lower = first.to_ascii_lowercase();
    if matches!(lower.as_str(), "at" | "every") && !after.is_empty() {
        // `at <datetime> <prompt>` or `every weekday 08:00 <prompt>` / `every 1h <prompt>`
        if lower == "at" {
            if let Some((_, body)) = after.split_once(char::is_whitespace) {
                return body.trim();
            }
            return after;
        }
        // every <interval|weekday ...>
        return strip_every_clause(after);
    }
    if is_compact_interval_token(first) && !after.is_empty() {
        return after;
    }
    t
}

fn strip_every_clause(after: &str) -> &str {
    let lower = after.to_ascii_lowercase();
    if lower.starts_with("weekday ") || lower.starts_with("weekdays ") {
        // every weekday HH:MM <prompt>
        let rest = after
            .split_once(char::is_whitespace)
            .map(|(_, r)| r.trim())
            .unwrap_or("");
        if let Some((_, body)) = rest.split_once(char::is_whitespace) {
            return body.trim();
        }
        return rest;
    }
    // every 1h <prompt> / every monday 08:00 <prompt>
    if let Some((_, body)) = after.split_once(char::is_whitespace) {
        let body = body.trim();
        // monday 08:00 <prompt> — skip clock token if present
        if let Some((clock, rest)) = body.split_once(char::is_whitespace)
            && looks_like_clock(clock)
        {
            return rest.trim();
        }
        return body;
    }
    after
}

fn is_compact_interval_token(s: &str) -> bool {
    if s.len() < 2 {
        return false;
    }
    let (digits, suffix) = s.split_at(s.len() - 1);
    matches!(suffix, "s" | "m" | "h" | "d")
        && digits.chars().all(|c| c.is_ascii_digit())
        && digits.parse::<u64>().is_ok_and(|n| n > 0)
}

fn looks_like_clock(s: &str) -> bool {
    let mut parts = s.split(':');
    let (Some(h), Some(m)) = (parts.next(), parts.next()) else {
        return false;
    };
    h.bytes().all(|b| b.is_ascii_digit()) && m.bytes().all(|b| b.is_ascii_digit())
}

pub const UPDATE_GOAL_TOOL_NAME: &str = "update_goal";

pub const WORKFLOW_TOOL_NAME: &str = "workflow";

pub const GOAL_COMMAND_NAME: &str = "goal";

/// Bare subcommand tokens reserved for goal lifecycle control rather than
/// being treated as an objective, matching the shell's /goal grammar.
pub const GOAL_RESERVED_SUBCOMMANDS: &[&str] = &["status", "pause", "resume", "clear", "edit"];

pub fn goal_usage_message() -> &'static str {
    "Usage: /goal <objective>\n\
     Set an objective to work toward until it is complete."
}

pub fn goal_instruction(objective: &str) -> String {
    format!(
        "# /goal -- pursue an objective\n\n\
         A goal has been set: {objective}\n\n\
         Work directly on this goal and carry it as far as you can. Deliver \
         everything the user asked for yourself: no follow-up questions, no \
         manual steps left for the user. If the conversation continues, keep \
         pursuing the goal until it is complete.\n\n\
         TRACKING: break the objective into concrete steps and track them \
         (use your todo tool if one is available), marking each done as you \
         finish it.\n\n\
         VERIFY AS YOU GO: test each change on the real path before moving on. \
         A completion claim must be backed by evidence produced in this \
         session, not assumptions.\n\n\
         Call update_goal(completed: true, message: \"summary\") ONLY when the \
         goal is fully achieved. Call update_goal(blocked_reason: \"reason\") \
         only when truly stuck after 3+ consecutive failed attempts at the \
         same problem. Call update_goal(message: \"status note\") to log \
         progress along the way. If update_goal returns an error, continue \
         working the goal and report status in your reply instead.\n\n\
         Start now."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imagine_instruction_carries_prompt_verbatim() {
        let text = imagine_instruction("a golden sunset");
        assert!(text.contains("a golden sunset"));
        assert!(text.contains("image_gen"));
        assert!(text.contains("verbatim"));
    }

    #[test]
    fn imagine_video_instruction_carries_prompt_and_workflow() {
        let text = imagine_video_instruction("a cat playing piano");
        assert!(text.contains("a cat playing piano"));
        assert!(text.contains("image_to_video"));
        assert!(text.contains("FFmpeg"));
    }

    #[test]
    fn instruction_carries_args_and_contract_tokens() {
        for mode in [LoopFireMode::Detached, LoopFireMode::InSession] {
            let text = loop_schedule_instruction("every 30 minutes do x", mode);
            assert!(text.contains("every 30 minutes do x"), "{mode:?}");
            assert!(text.contains("<number><unit>"), "{mode:?}");
            assert!(text.contains("ask the user how often"), "{mode:?}");
            assert!(
                !text.contains("10m"),
                "no host-side default interval: {mode:?}"
            );
            assert!(
                !text.contains("recurring:"),
                "the retired one-shot flag must not be referenced: {mode:?}"
            );
            assert!(
                text.contains("task_id"),
                "must teach in-place updates via task_id: {mode:?}"
            );
            assert!(
                text.contains("delete and recreate"),
                "must steer away from delete+recreate: {mode:?}"
            );
            assert!(
                text.contains("scheduler_delete <task_id>"),
                "every mode must authorize the fire to end the task: {mode:?}"
            );
        }
    }

    #[test]
    fn each_fire_mode_describes_its_own_runtime() {
        let detached = loop_schedule_instruction("5m check ci", LoopFireMode::Detached);
        let in_session = loop_schedule_instruction("5m check ci", LoopFireMode::InSession);

        assert!(detached.contains("cannot see this conversation"));
        assert!(!detached.contains("arrives as a new turn in this conversation"));

        assert!(in_session.contains("arrives as a new turn in this conversation"));
        assert!(!in_session.contains("cannot see this conversation"));

        // The two levers the A/B showed carry the behavior are mode-independent.
        for text in [&detached, &in_session] {
            assert!(text.contains("report it and call"));
            assert!(text.contains("Keep it short and concrete"));
        }
    }

    #[test]
    fn goal_instruction_carries_objective_and_contract_tokens() {
        let text = goal_instruction("ship the widget");
        assert!(text.contains("ship the widget"));
        assert!(text.contains("update_goal(completed: true"));
        assert!(text.contains("blocked_reason"));
        assert!(text.contains("If update_goal returns an error"));
        assert!(
            !text.contains("system-reminder"),
            "expansions ride as user messages and must not claim reminder authority"
        );
        assert!(goal_usage_message().contains("Usage: /goal"));
    }

    #[test]
    fn usage_message_has_no_default_claim() {
        assert!(loop_usage_message().contains("Usage: /loop"));
        assert!(!loop_usage_message().contains("10m"));
    }

    #[test]
    fn schedule_usage_lists_verbs_and_recipes() {
        let u = schedule_usage_message();
        assert!(u.contains("Usage: /schedule"));
        assert!(u.contains("/schedule list"));
        assert!(u.contains("search <query>"));
        assert!(u.contains("meeting join"));
        assert!(u.contains("Schedules/"));
    }

    #[test]
    fn parse_schedule_verbs() {
        assert_eq!(parse_schedule_verb("list"), ScheduleVerb::List);
        assert_eq!(parse_schedule_verb("show abc"), ScheduleVerb::Show { id: "abc" });
        assert_eq!(
            parse_schedule_verb("cancel abc123"),
            ScheduleVerb::Cancel { id: "abc123" }
        );
        assert_eq!(
            parse_schedule_verb("1h search rust"),
            ScheduleVerb::Create { rest: "1h search rust" }
        );
    }

    #[test]
    fn expand_search_recipe_writes_schedules() {
        let (prompt, title, meeting) = expand_schedule_recipe("search rust async");
        assert!(prompt.contains("Search the web for: rust async"));
        assert!(prompt.contains("Schedules/YYYY-MM-DD"));
        assert_eq!(title, "rust async");
        assert!(!meeting);
    }

    #[test]
    fn expand_meeting_join_forbids_start_process() {
        let (prompt, _, meeting) =
            expand_schedule_recipe("meeting join https://example.com/join Standup");
        assert!(prompt.contains("meeting_join"));
        assert!(prompt.contains("https://example.com/join"));
        assert!(prompt.contains("Start-Process"));
        assert!(meeting);
    }

    #[test]
    fn schedule_instruction_standing_and_no_inline() {
        let text = schedule_instruction("1h search rust async");
        assert!(text.contains("standing: true"));
        assert!(text.contains("durable: true"));
        assert!(text.contains("fire_immediately: false"));
        assert!(text.contains("Do NOT execute the prompt inline"));
        assert!(text.contains("1h search rust async"));
        assert!(!text.contains("auto-expire after 7 days"));
    }
}
