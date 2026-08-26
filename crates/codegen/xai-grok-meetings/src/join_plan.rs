//! Which transports `meeting_join` must try, in order.
//!
//! Microsoft Graph (`GROK_GRAPH_TOKEN`) is an optional **chat-as-operator**
//! fallback. It is not a notetaker join. A missing or failed Graph token must
//! still send a Teams web guest named "Turbo (Notetaker)" into the meeting so
//! Turbo can post to meeting chat. WASAPI/loopback is a last-resort capture of
//! this machine and puts nobody in the roster.

use crate::url::MeetingPlatform;

/// Whether a delegated Graph token is usable for chat-as-the-operator.
///
/// Never a prerequisite for seating the guest. [`plan_join`] treats every
/// variant the same for transport selection so a missing token cannot collapse
/// a Teams join to loopback-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphStatus {
    /// `GROK_GRAPH_TOKEN` is set.
    Configured,
    /// No Graph token — the common case.
    Missing,
    /// Token present but Graph join/chat already failed.
    Failed,
}

/// One step `meeting_join` may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinTransport {
    /// Teams anonymous **web** guest named "Turbo (Notetaker)".
    ///
    /// This is the path that puts Turbo in the lobby and in meeting chat.
    /// Independent of Graph. The bot navigates a web-join URL rewrite
    /// ([`crate::teams_web_join_url`]), not the desktop-app launcher.
    GuestWeb,
    /// Local capture of this machine (WASAPI loopback / mic). No in-meeting
    /// identity; chat Q&A through the notetaker is unavailable.
    LocalCapture,
}

/// Ordered attempts for one `meeting_join`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinPlan {
    /// First item is tried first. Guest web, when present, always precedes
    /// local capture.
    pub attempts: Vec<JoinTransport>,
}

impl JoinPlan {
    /// True when a Teams web guest will be dispatched (Graph or not).
    pub fn attempts_guest_web(&self) -> bool {
        self.attempts.contains(&JoinTransport::GuestWeb)
    }

    /// True when the only remaining option is this machine's audio.
    pub fn is_local_only(&self) -> bool {
        self.attempts == [JoinTransport::LocalCapture]
    }
}

/// Inputs that decide the join order. `graph` is accepted so callers cannot
/// "forget" it — and so tests can prove it does not change the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinPlanOpts {
    pub platform: MeetingPlatform,
    /// `GROK_MEETING_BOT` (defaults on).
    pub bot_enabled: bool,
    pub graph: GraphStatus,
}

/// Plan the transports for a join URL.
///
/// Teams + bot on → `[GuestWeb, LocalCapture]` even when Graph is missing or
/// failed. Anything else → `[LocalCapture]` (Zoom/Meet/Webex, or the bot
/// kill-switch). Graph never adds, removes, or reorders a transport.
pub fn plan_join(opts: JoinPlanOpts) -> JoinPlan {
    // Exhaustive on purpose: a new GraphStatus must be classified here rather
    // than silently becoming a gate on the guest.
    match opts.graph {
        GraphStatus::Configured | GraphStatus::Missing | GraphStatus::Failed => {}
    }

    if opts.bot_enabled && opts.platform == MeetingPlatform::Teams {
        JoinPlan {
            attempts: vec![JoinTransport::GuestWeb, JoinTransport::LocalCapture],
        }
    } else {
        JoinPlan {
            attempts: vec![JoinTransport::LocalCapture],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::url::{parse, teams_web_join_url};

    fn teams(graph: GraphStatus) -> JoinPlan {
        plan_join(JoinPlanOpts {
            platform: MeetingPlatform::Teams,
            bot_enabled: true,
            graph,
        })
    }

    /// fr_01a034d554ee7d61852456b9ff734cfd: Graph missing must still seat a
    /// web guest. Loopback-only leaves Turbo with no in-meeting identity, so
    /// `meeting_reply` cannot post to chat.
    #[test]
    fn graph_missing_still_attempts_guest_web_join_not_wasapi_only() {
        let plan = teams(GraphStatus::Missing);
        assert!(
            plan.attempts_guest_web(),
            "Graph missing must still dispatch Turbo (Notetaker): {plan:?}"
        );
        assert!(
            !plan.is_local_only(),
            "Graph missing must not collapse to WASAPI-only: {plan:?}"
        );
        assert_eq!(
            plan.attempts.first().copied(),
            Some(JoinTransport::GuestWeb),
            "web guest before local capture: {plan:?}"
        );
        let guest = plan
            .attempts
            .iter()
            .position(|t| *t == JoinTransport::GuestWeb)
            .expect("guest web");
        let local = plan
            .attempts
            .iter()
            .position(|t| *t == JoinTransport::LocalCapture)
            .expect("local fallback still allowed after the guest");
        assert!(
            guest < local,
            "WASAPI is the fallback, not the join: {plan:?}"
        );

        // The URL the notetaker navigates is the web-join rewrite, not a
        // desktop-app handoff — and that rewrite does not consult Graph.
        let url = parse("https://teams.microsoft.com/meet/2907709513066?p=abc").unwrap();
        let web = teams_web_join_url(&url).expect("short meet link is a web join");
        assert!(
            web.contains("anon=true"),
            "web guest must ask Teams for the anonymous client: {web}"
        );
    }

    #[test]
    fn graph_failed_is_the_same_plan_as_missing() {
        assert_eq!(teams(GraphStatus::Failed), teams(GraphStatus::Missing));
        assert_eq!(teams(GraphStatus::Configured), teams(GraphStatus::Missing));
        assert!(teams(GraphStatus::Failed).attempts_guest_web());
        assert!(!teams(GraphStatus::Failed).is_local_only());
    }

    #[test]
    fn bot_kill_switch_is_local_only_even_with_graph() {
        for graph in [
            GraphStatus::Configured,
            GraphStatus::Missing,
            GraphStatus::Failed,
        ] {
            let plan = plan_join(JoinPlanOpts {
                platform: MeetingPlatform::Teams,
                bot_enabled: false,
                graph,
            });
            assert!(plan.is_local_only(), "{graph:?} → {plan:?}");
            assert!(!plan.attempts_guest_web());
        }
    }

    #[test]
    fn non_teams_platforms_are_local_only_regardless_of_graph() {
        for platform in [
            MeetingPlatform::Zoom,
            MeetingPlatform::GoogleMeet,
            MeetingPlatform::Webex,
            MeetingPlatform::Other,
        ] {
            let plan = plan_join(JoinPlanOpts {
                platform,
                bot_enabled: true,
                graph: GraphStatus::Missing,
            });
            assert!(plan.is_local_only(), "{platform:?} → {plan:?}");
        }
    }
}
