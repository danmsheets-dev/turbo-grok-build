# The Offer — Free AI Search Readiness Audit
### Every post in this campaign points here. It does not exist yet. Build it in Day 0.

---

## 0. Why this needs building first

Your site currently offers three conversion paths: **Book a Strategy Call**, the **Revenue Leak Calculator**, and the **AI Lab**. There is no audit offer.

If you start posting before this page exists, you burn 30 days of attention into a generic "book a call" — which converts a cold audience at a fraction of the rate. **Do not start Day 1 until this is live.**

---

## 1. Offer specification

| | |
|---|---|
| **Name** | AI Search Readiness Audit |
| **Always subtitled** | Answer Engine Optimization (AEO) / Generative Engine Optimization (GEO) |
| **Price** | Free |
| **Turnaround** | 3 business days |
| **Delivered as** | A 6–9 page PDF + a 5-minute recorded walkthrough (Loom) |
| **Capacity limit** | **5 per week** — stated publicly |
| **Qualification gate** | B2B, has a real website, ≥10 pages of content |
| **URL** | `revenuedrivenai.com/ai-search-audit/` |
| **Natural next step** | The $3,500 GEO visibility sprint |

**Why free, capped, and 3-day:** free removes friction on a cold audience; the cap of 5/week creates scarcity *and* is honest about a 3-person firm's capacity; the fast turnaround proves operational competence, which is exactly what an automation buyer is evaluating.

**Why a PDF + video rather than an automated tool:** the video is the conversion mechanism. A prospect who watches a senior architect explain what's wrong with their site for five minutes is dramatically more likely to book. It also lets you disqualify bad fits without a call.

---

## 2. What the audit actually contains

Each section maps to a KPI your GEO service page already names — so the audit is a natural, non-salesy on-ramp to the paid sprint.

| # | Section | What you actually do | Deliverable |
|---|---|---|---|
| 1 | **Prompt visibility test** | Run 10 buyer-intent prompts through ChatGPT, Perplexity, Google AI Overviews, and Claude. Record whether they appear, and who appears instead. | Screenshot grid + a scored table |
| 2 | **Answer accuracy check** | Ask each engine "What does [company] do and who is it for?" Compare to their actual positioning. | Side-by-side: what AI says vs. reality |
| 3 | **Entity clarity score** | Is the org unambiguously identified? `Organization` schema, consistent NAP, `sameAs` links, About page, third-party mentions. | 0–10 score + specific gaps |
| 4 | **Schema & structured data** | Crawl for `Organization`, `Service`, `FAQPage`, `Article`, `BreadcrumbList`, `Product`. Validate. | Pass/fail table per template |
| 5 | **Answer-asset coverage** | Map the 15 questions their buyers ask before purchasing. Which have a dedicated page? | Coverage matrix — usually the most alarming page |
| 6 | **Retrievability & crawl** | `robots.txt` and AI-crawler rules (GPTBot, ClaudeBot, PerplexityBot, Google-Extended), JS-only rendering, page speed, `llms.txt` | Blockers list |
| 7 | **Competitive citation gap** | Who *is* getting cited for their category prompts, and what those pages do differently. | Named competitors + the specific pattern |
| 8 | **The 3 fixes** | Ranked by impact-over-effort. Specific, not generic. | Prioritized action list |

> **Section 5 is the emotional core.** Showing a marketing leader that their buyers ask 15 questions and their site answers 4 of them is what makes them book. Lead the video walkthrough with it.

---

## 3. Fulfillment — keep it under 90 minutes per audit

You are an automation company. The audit must *demonstrate* automation, not consume your week. Target: **≤90 min of human time**, most of it on the video.

| Step | Time | How |
|---|---|---|
| 1. Intake → trigger | 0 min | HubSpot form submission fires the workflow |
| 2. Automated crawl | 0 min (async) | Script pulls schema, meta, headings, `robots.txt`, sitemap, page inventory |
| 3. Automated prompt runs | 0 min (async) | Scripted queries against the answer engines; capture responses + screenshots |
| 4. Draft generation | 5 min | Findings → your PDF template |
| 5. **Human review + the 3 fixes** | **45 min** | The part that must be senior and real. Never automate the recommendations. |
| 6. Loom walkthrough | 15 min | Screen-share the PDF; lead with the coverage matrix |
| 7. Send + HubSpot sequence | 5 min | Deliver, log properties, enroll follow-up |

**Build note:** you already own the hard parts of this — a browser-automation layer, clean URL→markdown extraction, multi-provider model routing, and scheduled jobs. This audit is a natural product of that stack, and *saying so in the campaign* is itself a proof point: "we built the audit tooling on our own agent runtime."

**HubSpot properties to create:** `Audit Requested Date`, `Audit Entity Score`, `Audit Schema Pass Rate`, `Answer Coverage %`, `AI Answer Inclusion Count`, `Audit Delivered Date`, `Audit Video Watched`, `Top Fix Recommended`.

**Follow-up sequence:** Day 0 delivery → Day 3 "did the video make sense?" → Day 7 "here's the one fix I'd do first" → Day 14 sprint offer → Day 30 re-test one prompt for free.

---

## 4. Landing page copy — `/ai-search-audit/`

> Written to match your existing site voice: plain, technical, anti-hype, diagnose-first.

---

### HERO

**Eyebrow:** `FREE DIAGNOSTIC · 3 BUSINESS DAYS · 5 PER WEEK`

# Find out what AI tells buyers about your company.

Your next customer is asking ChatGPT, Perplexity, and Google's AI which vendors to evaluate. We'll show you whether you appear in that answer — and if you do, whether it's correct.

Free. Three business days. A real audit from a senior architect, not an automated score.

`[ Request my audit → ]`   `[ See what's inside ↓ ]`

*Currently accepting 5 audits per week.*

---

### THE PROBLEM BLOCK

## Your website was built for search engines that typed. Your buyers now ask.

For twenty years the job was to rank a page for a keyword. That job still exists — but a growing share of B2B research now starts as a question posed to an AI tool, and the answer names three vendors.

If your site does not make your category, services, buyer fit, and proof machine-readable, you can be an excellent fit and still be left out of the answer. **You won't see it in your analytics. There's no impression to lose — the conversation simply happened without you.**

---

### WHAT YOU GET

## Eight checks. One prioritized fix list.

| | |
|---|---|
| **01 — Prompt visibility test** | We run 10 real buyer-intent prompts through ChatGPT, Perplexity, Google AI Overviews, and Claude, and record whether you appear — and who appears instead. |
| **02 — Answer accuracy check** | We ask each engine what you do and who you serve, then show you the gap between that and your actual positioning. |
| **03 — Entity clarity score** | Whether AI systems can confidently identify your organization as a distinct, credible entity. Scored 0–10 with the specific gaps. |
| **04 — Schema & structured data** | A template-by-template pass/fail on the structured data that tells machines what each page is. |
| **05 — Answer coverage matrix** | The 15 questions your buyers ask before they purchase — and which of them your site actually answers on a dedicated page. |
| **06 — Retrievability & crawl access** | Whether AI crawlers can reach and parse your content at all, including the rules most sites set by accident. |
| **07 — Competitive citation gap** | Who is getting cited for your category prompts, and what their pages do that yours doesn't. |
| **08 — Your three highest-impact fixes** | Ranked by impact over effort. Specific to your site. Yours to keep, whether or not you ever work with us. |

**Delivered as:** a written audit (6–9 pages) and a 5-minute recorded walkthrough where a senior architect talks through what we found.

---

### HONESTY BLOCK

## What this is not.

**It is not an automated score.** Software runs the crawl and the prompt tests. A senior architect writes the findings and records the walkthrough. That's the part that matters.

**It is not a guarantee of citation.** No one can promise ChatGPT or Perplexity will cite you — anyone who does is selling something they can't deliver. We can tell you precisely what's missing and what to build.

**It is not a disguised sales call.** You get the fix list whether you hire us or not. If your site is already in good shape, we'll tell you that and you'll be done in five minutes.

---

### PROOF BLOCK

## We publish our work.

We're a small, private firm, and our partners don't keep public profiles. So we prove capability with artifacts instead of bios:

- **Public source code.** Our multi-agent AI runtime is open source under Apache-2.0 — a Rust codebase you can read, including a changelog that documents our own failures in detail.
- **Live tools.** The AI Lab runs real interactive lead-gen apps you can test before you spend a dollar.
- **This audit.** Same principle: we show you the diagnosis before we ever quote a build.

`[ Read the code on GitHub ]`   `[ Open the AI Lab ]`

---

### FORM

## Request your audit

`Work email*` · `Company website*` · `Company name*` · `Your role*`
`Which best describes you?*` — B2B SaaS / Technical services / Agency or consultancy / E-commerce / Other
`What would you most like to know?` (optional, free text)
`Name up to 3 competitors you'd like us to compare you against` (optional)

`[ Request my audit → ]`

*We'll confirm within one business day and deliver within three. We don't add you to a newsletter. Unsubscribe isn't necessary because there's nothing to unsubscribe from.*

---

### FAQ

**Is this really free?** Yes, and the fix list is yours to keep. We cap it at 5 per week because a senior person does the analysis. If you want the fixes implemented, that's a paid GEO sprint starting at $3,500 — but that's a separate decision you make later.

**How is this different from an SEO audit?** SEO audits measure whether you rank when someone types a keyword. This measures whether you're *named* when someone asks a question. Different content shape, different technical signals, different success metric. Both matter; most audits only cover the first.

**Do you need access to our site or analytics?** No. Everything is measured from the public web, the same way an AI crawler sees you.

**What if we're not on HubSpot?** Fine. The audit is platform-agnostic. HubSpot is where we do most implementation work, not a requirement for the diagnosis.

**How long until we'd see results if we fixed things?** Answer engines re-crawl and re-index on their own schedules. Realistically, expect meaningful movement over 4–12 weeks, and expect to re-test rather than assume. We'll show you how to re-test yourself.

---

### CLOSING CTA

## Find out what the answer engines are saying about you.

It takes two minutes to request and three days to receive. Then you'll know.

`[ Request my audit → ]`

---

## 5. Confirmation email (auto-send on submit)

**Subject:** Your AI Search Readiness Audit — what happens next

> Thanks — we've got your request for **{{company}}**.
>
> Here's the process, so there are no surprises:
>
> **Today:** our crawl and prompt tests start running against {{website}}.
> **Within 1 business day:** we confirm you're in this week's batch, or tell you the earliest slot.
> **Within 3 business days:** you get the written audit and a 5-minute video walkthrough.
>
> One thing that helps: if there's a specific prompt you wish you appeared for — the exact sentence a good-fit buyer might type into ChatGPT — reply with it and we'll add it to the test set.
>
> No newsletter, no drip sequence. Just the audit.
>
> — RevenueDrivenAI

---

## 6. Delivery email

**Subject:** {{company}}'s AI search audit — you appear in {{n}} of 10 buyer prompts

> Hi {{first_name}},
>
> Audit attached, and here's the 5-minute walkthrough: **{{loom_link}}**
>
> The short version:
>
> - You appeared in **{{n}} of 10** buyer-intent prompts we tested.
> - When AI tools describe what you do, they get it **{{accuracy_verdict}}**.
> - Your site answers **{{coverage_n}} of the 15 questions** your buyers ask before they buy.
> - Entity clarity score: **{{entity_score}}/10**.
>
> The single highest-impact fix: **{{top_fix}}**. Page 7 explains exactly how.
>
> All three fixes are yours to run with. If you'd rather we implement them, reply and I'll send scope — but there's no obligation and I won't chase you about it.
>
> — RevenueDrivenAI

---

## 7. Day-0 build checklist

- [ ] Create `/ai-search-audit/` page with the copy above
- [ ] Build the HubSpot form + the 8 custom properties
- [ ] Create the audit PDF template (8 sections, branded)
- [ ] Write the 10-prompt test set for your own top 3 verticals
- [ ] Script the crawl (schema, meta, headings, robots, sitemap, page inventory)
- [ ] Script the prompt runs with screenshot capture
- [ ] Set up the 5-email follow-up sequence
- [ ] Add `FAQPage` schema to the page itself *(you are selling AEO — this page must be exemplary)*
- [ ] Add `Service` schema for the audit
- [ ] Link it from the main nav, the GEO service page, and the site footer
- [ ] **Run the audit on your own site first** — you will need those numbers for Week 4, and you should not sell a diagnosis you haven't survived
