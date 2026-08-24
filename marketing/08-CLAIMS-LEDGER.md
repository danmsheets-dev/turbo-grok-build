# Claims Ledger
## Read this before you publish anything

This is the safety rail for the campaign. Everything below is grounded in what the repository and your website actually say. Its purpose is to keep you from making a public claim you cannot defend — because in a campaign whose entire premise is *"we're honest about what breaks,"* one over-claim does disproportionate damage.

The long-form version, with file-level evidence for every item, is in `09-PROOF-LIBRARY.md` §6. This file is the summary you actually need before posting.

---

## 🚨 Section 0 — Launch blockers

**Three things will break the campaign on Day 1 if you don't handle them. Check all three before you publish anything.**

### 0.1 — `v1.0.0-rc.10` is not published. Every rc.10 link in your README is a 404.

Your `VERSION` file reads `1.0.0-rc.10` and the README documents rc.10 download links, asset names, and a `--version v1.0.0-rc.10` pin command. But `gh release list` shows **v1.0.0-rc.9 as Latest**, and `gh release view v1.0.0-rc.10` returns *"release not found."*

A prospect clicking the download link in your first launch post hits a 404. That ends a launch's credibility faster than anything else in this document.

**Also affected:** four story posts (Days 2, 5, 19, and the Day 22 notetaker material) describe fixes that shipped *in the rc.10 changelog entry* but are not in any downloadable release.

**Before Day 1, do one of:**
- Publish the rc.10 release (you mentioned a release build is in flight — verify it actually tagged and published, with `SHA256SUMS`), **or**
- Say "1.0.0-rc.9" everywhere and narrate the rc.10 fixes as "in the next release."

> Verify with: `gh release view v1.0.0-rc.10`

### 0.2 — "Meeting audio never leaves the machine" is false. Do not say it.

The in-page tap streams PCM over `127.0.0.1`, but that socket feeds a loop that streams to **`wss://api.x.ai/v1/stt`**. Meeting audio is uploaded to xAI's cloud STT service.

The phrase originates in your own CHANGELOG, which contradicts itself in a single sentence — and your own design spec at `docs/superpowers/specs/2026-08-23-teams-guest-bot-design.md` lists *"Meeting audio must not transit a third-party SaaS"* as a hard constraint and rejects competitors for exactly that reason. Reading those two files together disproves the claim in about 30 seconds.

**I have already corrected this in the Day 1 and Day 22 copy.** The defensible version — which is still genuinely differentiated:

> "The tap is **in-page** rather than on your sound card, so it keeps transcribing with your speakers muted, your headset unplugged, or you gone entirely — with no virtual audio cable and no third-party notetaker vendor in the path."

If asked directly where the audio goes: **xAI's streaming STT service.** Answer it plainly.

### 0.3 — "Every agent runs in an isolated worktree" is false.

`resolve_default_isolation()` returns `None` for the `explore`, `plan`, and `oracle` agent types, and for any read-only capability mode. Three of four built-in agent types default to the shared workspace, and named tests assert exactly that. Your own user-guide doc makes this error too — so a skeptic reading the code finds your docs and your code disagreeing, which is worse than the original overclaim.

**Corrected in the copy to:** *"Write-capable subagents get their own git worktree by default. Read-only research agents skip it — they ship with no shell or editing tools at all."*

That's a better sentence anyway: it shows the permission model is deliberate rather than blanket.

---

## 🔴 Section 1 — Fix these in the repo before Day 1

These are live inconsistencies that a curious prospect, a competitor, or a Hacker News commenter can find in under five minutes.

| # | Issue | Where | Why it matters | Fix |
|---|---|---|---|---|
| 1 | **`SECURITY.md` routes vulnerability reports to xAI's bug bounty** (`hackerone.com/x`) | [SECURITY.md](../SECURITY.md) | It misroutes real reports about *your* fork to a company that didn't write it, and it implies an affiliation your README explicitly disclaims. This is the most damaging of the three — it's a live promise to security researchers that you can't keep. | Replace with your own contact: a security email and a disclosure window. |
| 2 | **`NOTICE` is still branded "Hyper"** — "Copyright 2026 Hyper contributors" | [NOTICE](../NOTICE) | Stale attribution to a different fork lineage. Reads as inattentive on a project whose pitch is rigor. | Update to your own entity name. Keep the upstream Apache-2.0 attribution paragraph exactly as it is. |
| 3 | **`docs/KNOWN_ISSUES.md` opens with "# Hyper known issues"** | [docs/KNOWN_ISSUES.md](../docs/KNOWN_ISSUES.md) | Same problem, and this file is one you'll actively point people to in Week 4. | Retitle. |
| 4 | **Repo has no topics set and a 1-line developer-only description** | GitHub settings | You're about to drive traffic to it. It's configured like a private side project. | See `06-SUPPORTING-CHANNELS.md` §7. |

---

## 🔴 Section 2 — The trademark question

**The product name contains "Grok," which is an xAI brand name. Apache-2.0 does not grant you the right to use it.**

Section 6 of the license is explicit: *"This License does not grant permission to use the trade names, trademarks, service marks, or product names of the Licensor."*

The repo already carries the right disclaimers, and it uses them consistently:

> **"Not affiliated with xAI. Based on Apache-2.0 Grok Build source."**
> **"Turbo Grok Build is an independent community fork — not an official xAI product."**
> **"…is an independent community build and is not affiliated with, endorsed by, or sponsored by xAI / SpaceXAI or Moonshot AI."**

That's appropriate for a community fork. But the risk profile changes when **a company begins promoting it commercially from a brand account as evidence of its capabilities.** That is a different activity from publishing a hobby fork, and it's the activity this campaign is built on.

**Recommendations, in order of preference:**

1. **Rename the product** to something with no xAI mark — e.g. *"Turbo Build"* or a name of your own. It costs a release and removes the question entirely. Given you're about to attach a commercial brand to it, this is the clean answer.
2. **Have counsel look at it** before the campaign, if you'd rather keep the name.
3. **At minimum:** carry the non-affiliation disclaimer in *every* post that names the project. This is already a rule in the copy — do not drop it to save characters.

I'm not a lawyer and this isn't legal advice. It's a flag that a reasonable person would want raised before spending 30 days linking their company to this name.

---

## 🟠 Section 3 — The anonymity conflict

Your About page states, deliberately:

> "Our partners currently lead architecture and strategy at major enterprise software companies. Connecting those roles to client work publicly could compromise the independence our clients rely on — and the firewall our partners need to maintain."

**The repo's git history contains a real name and a personal email address across 211 commits, under a personal GitHub account.**

Promoting it from the RevenueDrivenAI brand account creates a public, permanent, machine-readable link between the company and that individual. Rewriting git history to remove it is possible but disruptive and incomplete (forks, caches, and the GitHub API retain copies).

**Decide deliberately, before Day 1:**

- If that identity is **not** one of the firewalled partners → proceed; consider a company GitHub org anyway for the entity signal.
- If it **is** → create a `revenuedrivenai` org and publish a **fresh mirror** with squashed history under an org-owned identity, and don't link the original.

---

## 🟡 Section 4 — Self-reported vs. independently verifiable

The distinction matters more than usual here, because your campaign's whole credibility rests on precision.

| Claim | Status | How to say it | How **not** to say it |
|---|---|---|---|
| ~28,400 tests passing | **Self-reported** in your own changelog | "our test suite runs [N] tests" | "independently verified" / "28,414 tests prove…" |
| ~2,687 Rust source files | **Verifiable** — anyone can count | "a few thousand source files" or the exact count | inflating it into "lines of code" |
| Apache-2.0, public since 2026-07-29 | **Verifiable** | freely | — |
| 371 commits | **Verifiable, but read the split** | see Section 5 | "371 commits of our work" |
| Security issues found and closed pre-release | **Self-reported** | "our own pre-release audit found and closed X before shipping" | "third-party security audited" / "penetration tested" / any severity rating you didn't get from an external assessor |
| **The 47-agent self-audit** (see box below) | **Self-reported but documented in detail** | "we ran a 47-agent adversarial audit over 104 changed files and it told us the release wasn't ready" | "audited" without saying it was your own process |
| CI is green | **Verifiable** via the public badge | "CI runs on every push" | "fully tested" |
| Ten release candidates on the 1.0 line | **Verifiable** via release tags | freely | — |

**Rule:** when a number comes from your own documentation, say so in the sentence. *"Our test suite reports 28,414 passing tests"* is bulletproof. *"28,414 tests, independently verified"* is a lie you'd never survive being asked about.

### ⚠️ Your two test counts disagree

Two documents in the repo report different figures, from different scopes and dates:

| Source | Figure |
|---|---|
| [README.md:163](../README.md) | "**28 414 tests pass** (`cargo test --workspace --lib` is fully green)" |
| [docs/RC2_UNRELEASED_AUDIT.md](../docs/RC2_UNRELEASED_AUDIT.md) baseline | "27,576 passed / 7 failed / 56 ignored" |

Neither is wrong — different snapshots. But if you publish one and a reader finds the other, the precision that makes this campaign credible is what takes the damage.

**Do one of these before Day 1:**
- Re-run the suite, publish the current number, and make both docs agree; **or**
- Say **"roughly 28,000 tests in our own suite"** in all social copy and skip the false precision.

The second option is safer and costs you nothing rhetorically.

### The `--features community-build` caveat

Your own audit notes that the **shipped** binary is compiled with a feature flag that the test run did *not* exercise, and states plainly: *"the shipping configuration is less covered than the default one."*

Never say "every line that ships is tested." Your own documentation contradicts it. Say "our test suite runs on every push" and leave it there.

### ⭐ The strongest proof point in the repo — use it

[`docs/RC2_UNRELEASED_AUDIT.md`](../docs/RC2_UNRELEASED_AUDIT.md) is the best marketing asset you own, and the campaign should lean on it harder than on any feature.

What it documents, verbatim:

> **"47 agents in two passes — 9 area-scoped finders, adversarial refutation of every high/medium finding, a completeness critic, then a payload-level adjudication round for the disputed security items. Every severity below survived at least one attempt to refute it."**

Scope: 104 files, +13,927 / −337 lines. And the conclusion:

> **"Verdict: not ready."**

**You ran a 47-agent adversarial audit on your own release, it told you the release wasn't ready, and you listened.** That is a better story than any capability post — it demonstrates the exact discipline a buyer wants from someone touching their CRM: an independent check with the authority to say no, and a team that honours it.

It also contains genuine self-criticism (*"the shipping configuration is less covered than the default one"*), which is precisely the tone this campaign is built on.

**Reframe Day 16 around this.** "We found a way out of our own sandbox" is true but narrow. *"Our own audit blocked our own release"* is stronger, more defensible, and lands harder with a non-technical buyer.

**One caution:** it is *your* audit of *your* code. Always say so. "We audited ourselves and published the result" is impressive and honest. "Audited" on its own implies a third party and would be misleading.

---

## 🟡 Section 5 — The honest state of traction

Do not imply community adoption you don't have.

| Reality | As of 2026-08-24 |
|---|---|
| GitHub stars | **1** |
| Forks | **0** |
| Repo age | ~4 weeks public |
| Commits | 371 total |
| **Commit authorship split** | ~211 from your account; ~149 from upstream snapshots; ~130+ from the Hyper community fork; the rest from other contributors |

**That last row is the one to be careful with.** This project is a fork of a fork — genuine work sits on top of substantial inherited code. That is completely normal and nothing to hide, but describing it as *"we built a multi-agent AI runtime"* without the word *fork* is the claim most likely to get you publicly corrected, and it's the objection Hacker News will raise first.

**Say:** *"We extended xAI's open-source Grok Build with a multi-agent layer — isolated agent workspaces, self-verifying audit workflows, browser and meeting automation."*
**Don't say:** *"We built a multi-agent AI system from scratch."*

Owning the fork status is a *strength* in this campaign. It's consistent with the honesty positioning, and it pre-empts the criticism.

---

## 🟡 Section 6 — Features that are not fully validated

Your own `docs/KNOWN_ISSUES.md` is admirably explicit about this — which is exactly why you must not contradict it in marketing. It says, verbatim:

> **"Do not read a green test suite as a validated fix — the unit tests assert the wiring, not the effect."**

**Do not claim as proven:**

| Feature | The repo's own status |
|---|---|
| Teams web-join URL rewrite | **Explicitly a guess** — derived from one observed redirect chain, not documentation. Ships behind a kill switch (`GROK_MEETING_TEAMS_WEB=0`). |
| Download-blocking during meeting join | **Explicitly a guess** — no pinned DevTools protocol version; failure only warns. |
| Teams DOM selectors | Unvalidated against a live meeting; expected to break when Teams ships UI changes. |
| The meeting notetaker generally | Known limitation: a meeting opened in a *new tab* is invisible to the bot, which then polls an abandoned page until timeout. |
| Zoom / Meet / Webex notetaking | **Only Teams gets a bot.** The others fall back to local capture. Never imply otherwise. |
| Speaking in a meeting | **Not possible** — no text-to-speech. It answers in chat only. |

**Marketing translation:** talk about the meeting notetaker as *an example of browser automation under agent control*, and as a *design* story about consent and honest failure reporting. Do not present it as a polished, battle-tested product feature. Day 22's post is written accordingly.

### More claims the audit killed

| Don't say | Why | Say instead |
|---|---|---|
| The write confinement is a **"sandbox"** or **"jail"** | Your own `KNOWN_ISSUES.md` says *"Shell confine is not an OS sandbox"* and *"policy-level, not OS FS jail."* Several allowed programs execute arbitrary code — `cargo run` writes anywhere. Someone with a Rust toolchain demos the escape in 30 seconds. | "A fail-closed **write boundary** — a policy jail, not an OS sandbox. Writes resolving outside the root are denied at a single chokepoint." *(Already corrected in Day 16/17 copy.)* |
| "Ships with five stock workflow recipes" | Grepping the crate tree for those recipe names returns **zero hits**. They aren't embedded, registered, or packaged by the release workflow. Two contain repo-specific prompt text a skeptic would screenshot. | "**Three** workflows are built into the binary: `deep-audit`, `deep-research`, `continuous-improve`." |
| Any authorship claim over **`/goal`** or the adversarial completion verifier | `git log` on those files returns only upstream bot commits and one sync merge. **It's upstream xAI code, not yours.** One `git log` disproves it. | Don't feature it as a fork innovation at all. |
| "Every commit is lint-clean and tested in CI" / "CI-verified" | CI runs **no** clippy, **no** rustfmt, and **no** full workspace test suite — `release.yml` contains a comment explaining why workspace tests were deliberately *removed* as a gate. `keep-features.yml` also pins Rust 1.93.0 while `rust-toolchain.toml` declares 1.94.0, so the gate compiles differently from the release. | Claim CI coverage only for what `keep-features.yml` names. Everything else is "unit-tested," not "CI-verified." |
| "The browser layer is field-hardened / CI-tested" | No Windows test job exists in any workflow. The WebView2 path is compiled but never executed in CI. `docs/BROWSER-R3-QA.md` is a 50-row checklist with **empty** result columns — including the password-refusal row marked *"Do not skip for ship."* | "Unit-tested at the tool and policy layer; the live path is verified by hand on Windows." |
| "No competing coding CLI ships a meeting bot" | Unverifiable from your repo, and a competitive claim you can't source. | Drop it, or "we're not aware of another one that does this." |
| Publishing a **live Teams join demo or screenshot** | `KNOWN_ISSUES.md` opens with the heading *"Unvalidated against a live meeting."* All 14 selector tests are string assertions over JavaScript source text. There is no evidence in the repo of a confirmed successful admit into a real meeting. | Don't publish a notetaker demo until you have personally reproduced a live join and captured it. |

---

## 🔴 Section 7 — The meeting bot: consent and compliance

This is the highest-risk content in the campaign, and it's also some of the most interesting. The rule is simple: **lead with the consent design, never with the capability.**

**What the repo actually does (all of this is good, and all of it is the story):**
- Joins as a **clearly named participant**: "Turbo (Notetaker)."
- **Waits in the lobby** and must be **explicitly admitted**, like any other attendee.
- Its microphone is **silent by construction** (a zero-gain audio node), and it never reaches the operator's real microphone.
- Meeting audio is processed **on the machine** — no third-party meeting service.
- If a tenant's policy blocks external bots, it **reports the refusal and stops.** The repo states it "surfaces that rather than working around it."
- It **never answers a verification challenge**. A challenge ends the join.
- Coworker questions are treated as **untrusted input from potentially external participants with spoofable display names**, and are confined to read-only tools *enforced at dispatch*, failing closed if unclassifiable.

That last point is genuinely excellent security design and worth a post on its own.

**Never write, imply, or joke about:**
- Recording people without their knowledge
- Evading bot detection, or a tenant's external-bot policy
- Joining meetings you weren't invited to
- "Silent" or "invisible" notetaking

**Anticipate and answer proactively:** two-party consent laws vary by jurisdiction; recording is the meeting owner's responsibility; the bot is designed to be visible and refusable *precisely so* that consent is possible. If you can say that before someone asks it, the post converts instead of backfiring.

---

## 🟠 Section 8 — AEO/GEO claim limits

Your own GEO page already sets the right boundary — do not let social copy exceed it.

**Never claim:**
- That you can guarantee a citation in ChatGPT, Perplexity, Google AI Overviews, or any engine
- A specific ranking, position, or share of voice
- A specific revenue or pipeline outcome from GEO work
- A timeline for when an engine will re-crawl or re-index (you don't control it)
- That AEO/GEO "replaces" SEO

**Safe framings:**
- "We identify which signals are missing and what to build."
- "Leading indicators we build toward and report against. Not guarantees."
- "Answer engines re-index on their own schedules. Expect to re-test rather than assume."
- "Here's what changed for us, measured with the same prompt test, before and after."

**On your own results (Days 26 and 29):** report the real numbers, including the prompts where you *still* don't appear. A case study with a failure in it is more persuasive than a clean one, and it's the only version consistent with this campaign's voice.

---

## 🟡 Section 9 — Universal rules

1. **Never publish a `[BRACKET]` placeholder.** Fill it with a real number or delete the sentence. Never invent one.
2. **Never imply client results you don't have.** You have no public case studies. Until you do, the proof is the code, the tools, and your own audit — which is plenty.
3. **Never name a client** without written permission.
4. **Never claim certification.** Your site says *"aligned with"* ISO/IEC 27001 and 42001 and *"follows"* NIST AI RMF. Keep exactly that wording. "Aligned with" and "certified" are different words with different legal weight.
5. **Never post a competitor comparison you can't source.** Naming a competitor as worse invites a response you'll lose.
6. **Never let AI-written copy ship unread.** In a campaign about AI honesty, an AI-generated error is the worst possible own goal.
7. **When corrected publicly, concede immediately and specifically.** Your entire positioning is that you admit what broke. A defensive reply costs more than the original error.

---

## ✅ Section 10 — Pre-approved wordings

Copy these exactly.

**On the repo:**
> "Our multi-agent AI runtime is open source under Apache-2.0. It's an independent community fork of xAI's Grok Build — not affiliated with xAI — extended with isolated agent workspaces, self-verifying audit workflows, and browser automation."

**On rigor:**
> "Our own test suite reports [N] passing tests, and our pre-release audit process found and closed issues before they shipped. We publish the ones we found, and the ones we're still not sure about."

**On the audit offer:**
> "A free AI Search Readiness Audit: we run ten buyer-intent prompts across ChatGPT, Perplexity, Google AI Overviews and Claude, check whether AI describes you accurately, score your entity clarity and schema, and map which of the questions your buyers ask actually have a page. Three business days, five per week, and the fix list is yours either way."

**On guarantees:**
> "No one can guarantee an AI tool will cite you, and anyone who does is selling something they can't deliver. We can tell you precisely what's missing."

**On the firm:**
> "We're a small private firm and our partners don't keep public profiles. So we prove capability with artifacts instead: public source code, live tools you can test, and a diagnosis before we ever quote a build."
