# Supporting Channels — beyond LinkedIn and X

LinkedIn and X carry the campaign. These six surfaces multiply it. Ranked by expected contribution.

---

## 1. Your own blog — the AEO engine 🔴 CRITICAL

**This is more important than any social channel, and it is currently your weakest asset.**

Your blog is four pages of *"Daily AI Roundup: OpenAI Launches…"* news aggregation. Understand what that costs you:

- It builds entity authority for **"OpenAI news"** — a topic you don't sell and can't win.
- It builds **zero** authority for "HubSpot AI automation," "AI lead routing," "GEO agency," or any phrase a buyer would actually ask about.
- It is the first thing a sharp prospect will notice when evaluating a company that *sells* AI search visibility.

You are a GEO agency whose blog is not built to be cited. Fix it during Week 1 and it becomes a story you can tell.

### The fix: answer assets, not articles

Every social post in this campaign has a companion page. Each one follows the same structure:

```
H1 = the exact question a buyer would ask
    ↓
40–60 word direct answer immediately below the H1   ← this is the extractable block
    ↓
Supporting detail, specifics, a table or numbers
    ↓
"Related questions" internal links (3–5)
    ↓
FAQPage schema + Article schema + author/org attribution
```

**Why the 40–60 word block matters:** retrieval systems lift self-contained passages. A paragraph that answers the question completely without needing the ones around it is dramatically more liftable than one that depends on context above it. This is the single highest-leverage on-page technique in AEO, and it is the thing you will teach in the audit.

### The 20 answer assets to publish (matched to the post calendar)

| # | Page H1 |
|---|---|
| 1 | What is an AI automation layer for a revenue team? |
| 2 | How do you connect HubSpot to an AI workflow? |
| 3 | What is speed-to-lead and how much revenue does slow follow-up cost? |
| 4 | What is Generative Engine Optimization (GEO)? |
| 5 | What is Answer Engine Optimization (AEO) and how is it different from SEO? |
| 6 | How do you know if ChatGPT recommends your company? |
| 7 | Which schema markup matters for AI search visibility? |
| 8 | Should you block or allow GPTBot, ClaudeBot, and PerplexityBot? |
| 9 | How do AI answer engines decide which companies to cite? |
| 10 | What is an AI agent, and where should a business actually use one? |
| 11 | How do you keep an AI workflow from corrupting CRM data? |
| 12 | What does human-in-the-loop actually mean in an automated workflow? |
| 13 | How do you test whether an AI automation is working? |
| 14 | What does an AI lead qualification workflow look like end to end? |
| 15 | How much does AI marketing automation cost for a B2B company? |
| 16 | What is NIST AI RMF and why should a marketing team care? |
| 17 | Can AI automation work with messy CRM data? |
| 18 | What should be automated first in a B2B revenue operation? |
| 19 | How long does GEO take to show results? |
| 20 | How we made ourselves visible to AI answer engines in 30 days *(the Day 30 case study)* |

**Cadence:** 3–4 per week, published the same day as the matching social post.

**Also add:** an `llms.txt` at your root summarizing what RevenueDrivenAI is, your services, and your canonical page URLs. Cheap, quick, and increasingly respected by retrieval systems. It also makes an excellent post.

---

## 2. AI-citable directories and listicles 🔴 CRITICAL

**The highest-ROI AEO tactic almost nobody executes deliberately.**

When someone asks an AI tool "who are the best HubSpot AI automation agencies?", the model rarely invents an answer from raw training data — it retrieves and synthesizes third-party list pages. **You cannot optimize your way into that answer from your own website.** You have to be *on the lists that get retrieved.*

### Do this in Week 1 — 2 hours

| Target | Action |
|---|---|
| **HubSpot Solutions Directory** | Highest priority. Directly relevant, heavily retrieved for HubSpot vendor questions. |
| **Clutch, G2, TrustRadius** | Create/claim profiles. Even unreviewed profiles get retrieved for "agencies that do X". |
| **Crunchbase** | Strong entity signal — feeds `sameAs`, disambiguates your org. |
| **GitHub organization** | A public org page with a real README is a genuine entity signal (see `01-STRATEGY.md` §6). |
| **LinkedIn company page** | Mandatory. Also an entity anchor. |
| **Wellfound / Product Hunt (org profile)** | Cheap additional entity coverage. |
| **Existing "best GEO agency / best AI marketing agency" listicles** | Find the top 15 ranking for those queries. Email each author with a specific, useful reason to include you. Expect a 10–20% hit rate — that's excellent for this. |

### Then close the loop

Add **`sameAs`** links in your `Organization` schema pointing to every profile you just created, and link them from your site footer. Right now your footer has no social links at all — that is a missing entity signal on a site selling entity clarity.

---

## 3. Reddit 🟠 HIGH

Two distinct audiences. Never cross the streams.

### Buyer subreddits
`r/HubSpot` · `r/RevOps` · `r/marketing` · `r/SaaS` · `r/B2BSaaS` · `r/DigitalMarketing`

**Rules:** answer three questions helpfully for every one time you mention yourself. Never link the offer in a top-level post. Your username should read as a practitioner, not a brand.

**What works:** long, specific, technical answers to "how do I automate X in HubSpot" questions. Give the complete answer including the parts that argue against hiring anyone. People DM the person who obviously knows, and those DMs convert far better than any post.

**What gets you banned:** posting the audit link, "great question!" openers, and anything that reads like it came from a content calendar.

### Amplifier subreddits
`r/rust` · `r/LocalLLaMA` · `r/ChatGPTCoding` · `r/AI_Agents`

For the repo. Share the *engineering stories*, not the product. r/rust in particular will engage with a specific, well-written debugging writeup and will punish anything that smells like promotion.

**Cadence:** 3 substantive comments per week + 1 post per week. Roughly 45 min/week.

---

## 4. Hacker News — one Show HN 🟡 MEDIUM, HIGH VARIANCE

**One shot. Day 17. Tuesday–Thursday, 8–10am ET.**

HN can produce 500+ stars and a genuine credibility spike, or it can produce a thread about how your project is "just a fork." Both outcomes are survivable; only one is likely if you frame it wrong.

### Framing rules

1. **Lead with the honesty, not the pitch.** HN's immune response is to marketing language. The strongest possible framing is a specific engineering story with the failure included.
2. **Say it's a fork in the title.** Trying to obscure it is the single fastest way to lose the thread. Owning it converts your biggest objection into a credibility signal.
3. **Do not link the audit offer.** Not in the post, not in a comment. HN will find your site on their own, and the ones who matter will.
4. **Be in the comments for the first four hours.** Answer every technical question in detail, concede every fair criticism immediately, and never get defensive. The comment thread is the actual product being judged.

### Recommended title

> `Show HN: A multi-agent coding CLI where every agent gets its own git worktree`

Alternative, if you'd rather lead with a story (often outperforms a Show HN):

> `The "large paste crash" that was actually a byte-offset bug on smart quotes`

### Post body

> This is a fork of xAI's Grok Build (Apache-2.0) that I've been extending for about a month. Not affiliated with xAI.
>
> The part I think is actually interesting: every subagent runs in its own git worktree, and merges land as `baseline..snapshot` diffs rather than "dirty working tree vs HEAD." That distinction turned out to matter more than I expected — without it, one agent's uncommitted work contaminates every other agent's diff, and "land this agent's changes" silently becomes "land everything on the machine."
>
> Also in here: a `/deepaudit` workflow that runs parallel investigators and then verifies each finding with an independent agent before reporting it, a meeting notetaker that joins Teams as a guest participant, and a scheduler.
>
> The changelog is unusually blunt about what broke — including a release that shipped a browser feature with a four-in-five chance of hanging, and a bug where a failed meeting join reported success. Happy to go into detail on any of it.
>
> [repo link]

---

## 5. Dev.to / Hashnode 🟡 MEDIUM

Republish the Week 2 engineering stories as full technical articles, 800–1,500 words each. Canonical-link them back to your site.

**Why bother:** both platforms are heavily crawled and frequently retrieved by AI tools for technical questions. An article titled *"Why your AI agent's 'success' message might be lying to you"* can get retrieved for years.

**Cadence:** 1/week, Weeks 2–4. Reuse the LinkedIn story posts as the skeleton — expand the technical detail, keep the business lesson as the conclusion.

---

## 6. Short video 🟡 MEDIUM

60–90 second screen recordings, no face required (which suits your anonymity constraint perfectly).

| Video | Shows |
|---|---|
| The Revenue Leak Calculator, end to end | What an interactive lead-gen asset feels like |
| Multiple AI agents working in parallel | The runtime, visually — genuinely striking to a non-technical viewer |
| Running a prompt visibility test live | The audit's core mechanic — best sales asset of the six |
| A HubSpot workflow firing from a form submit | The automation you actually sell |
| The audit deliverable, page by page | Removes all uncertainty about what they're requesting |
| Blocking vs. allowing AI crawlers in `robots.txt` | A concrete, teachable 60-second fix |

Post natively to LinkedIn (never as a YouTube link — LinkedIn suppresses off-platform links), and to X. Upload to YouTube with a keyword-rich title and a full description for the AEO surface.

---

## 7. GitHub as a marketing surface 🟠 HIGH

The repo is your proof asset. Right now it is configured like a private side project: **no topics set, 1 star, and a description written for developers only.**

### Day 0 fixes

**Repository topics** (currently empty — this is how GitHub search and external crawlers categorize you):
```
ai-agents · multi-agent · llm · agentic-ai · rust · cli · developer-tools
ai-automation · coding-agent · terminal · workflow-automation · browser-automation
```

**Description** — add the human hook:
```
Multi-agent terminal coding CLI in Rust. Isolated git worktrees per agent,
self-verifying audit workflows, browser + meeting automation. Apache-2.0.
```

**README additions:**
- A "Who builds this" section linking to RevenueDrivenAI — the only commercial mention, kept understated. Developers tolerate an honest attribution; they reject a pitch.
- A 20-second animated GIF near the top. The single highest-impact README change you can make — most people decide from the first screenful.
- Keep the xAI disclaimer exactly where it is.

**Ongoing:**
- Write real GitHub Release notes for every release — they're indexed and crawlable.
- Enable Discussions; a repo with activity reads as alive.
- Pin the 3 most interesting issues.

---

## 8. Weekly time budget

| Activity | Time |
|---|---|
| Schedule the week's LinkedIn + X posts (pre-written) | 60 min |
| Publish 3–4 answer assets | 120 min |
| Reddit engagement | 45 min |
| Reply to comments and DMs across platforms | 60 min |
| One Dev.to article | 45 min |
| One video | 30 min |
| Friday review | 30 min |
| **Total** | **~6.5 hrs/week** |

Plus audit fulfillment at ~90 min each. At 3–5 audits/week that's another 4.5–7.5 hours — and that portion is billable-adjacent work that directly produces pipeline, not overhead.
