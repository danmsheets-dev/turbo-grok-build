# Measurement — what to track, and what to ignore

---

## 1. The one number

**Audit requests per week.**

Everything else is a leading indicator or vanity. At a $3,500 sprint and a $2,000/mo retainer, one closed client returns the month many times over. A post with 40,000 impressions and zero audit requests is a failed post. A post with 900 impressions and three audit requests is the best post of the campaign.

Write this on a sticky note. On Day 12, when a story post does 20k impressions and you feel great, it will keep you honest.

---

## 2. Metric tiers

### Tier 1 — Pipeline (review weekly, act on)

| Metric | Where | Day 30 target |
|---|---|---|
| Audit requests | HubSpot form submissions | 12–25 |
| Audits delivered | HubSpot `Audit Delivered Date` | ≥90% of requests |
| Audit → call conversion | HubSpot deal stage | ≥35% |
| Strategy calls booked | Calendar | 4–8 |
| Closed engagements | HubSpot | 1–2 |
| Cost per audit request | Hours ÷ requests | Track it; you'll need it to decide about ads later |

### Tier 2 — Audience (review weekly, adjust content)

| Metric | Where | Day 30 target |
|---|---|---|
| LinkedIn followers | Page analytics | 200–400 |
| LinkedIn impressions | Page analytics | 25k–50k |
| LinkedIn engagement rate | Page analytics | >4% (below 2% = wrong topic or wrong hook) |
| **Profile visits from posts** | Page analytics | The truest attention signal on LinkedIn |
| X followers | X analytics | 150–300 |
| X profile clicks | X analytics | Track; this is X's equivalent signal |
| GitHub stars | Repo | 40–120 |
| Site sessions from social | GA4 | 600–1,500 |

### Tier 3 — AEO (review at Day 30 — this is the case study)

| Metric | How to measure | Day 30 target |
|---|---|---|
| **AI answer inclusion** | Re-run your 10 baseline prompts, count appearances | 3–6 of 10 |
| **Answer accuracy** | Ask 4 engines "what does RevenueDrivenAI do?" and score the description | Correct category + correct ICP |
| Answer assets published | Count of new answer-first pages | 18–20 |
| Pages with valid schema | Rich Results Test | 100% of new pages |
| AI crawler hits | Server logs — GPTBot, ClaudeBot, PerplexityBot, Google-Extended, Bytespider | Rising trend |
| Referral traffic from AI tools | GA4 referrer: chatgpt.com, perplexity.ai, claude.ai | >0 is a win at Day 30 |

> **Do this on Day 0, before you publish anything: run the 10 baseline prompts and screenshot every result.** Without the "before," you have no case study on Day 30 — and the case study is worth more than the campaign's direct leads. It is the single highest-value 45 minutes in this entire plan.

### Deliberately ignored

Likes. Comment counts on their own. Follower count as a goal. Impressions without profile visits. GitHub stars as anything other than a credibility prop. None of these pay you.

---

## 3. UTM scheme

Consistent tagging or the reporting is worthless.

```
?utm_source={linkedin|x|reddit|hn|devto|youtube|github}
&utm_medium=social
&utm_campaign=launch30
&utm_content=d{DAY}-{arc}
```

Examples:
```
https://revenuedrivenai.com/ai-search-audit/?utm_source=linkedin&utm_medium=social&utm_campaign=launch30&utm_content=d04-c
https://revenuedrivenai.com/ai-search-audit/?utm_source=x&utm_medium=social&utm_campaign=launch30&utm_content=d17-b
```

`utm_content` uses the day number and the post arc (**a** = proof, **b** = lesson, **c** = offer). At the end of the month you can answer: *which arc actually produced audit requests?* That answer shapes the next 90 days.

**Bare-link rule for the repo:** when you link GitHub in a post, use the clean URL with no UTMs — tracking params on a GitHub link look promotional and depress developer clicks. Track those with a separate short link instead.

---

## 4. Tracking setup (Day 0)

- [ ] GA4 goal on the audit form submission
- [ ] HubSpot form → contact property `Lead Source Detail` populated from UTMs
- [ ] LinkedIn Insight Tag installed (for retargeting later, even if you're not running ads yet)
- [ ] Server-log filter or GA4 exploration for AI crawler user-agents
- [ ] A simple sheet: `date | platform | day# | arc | post title | impressions | engagements | profile visits | link clicks | audit requests`
- [ ] **Baseline prompt screenshots taken and filed** ← do not skip
- [ ] Run your own AI Search Readiness Audit on `revenuedrivenai.com` and save the results

---

## 5. Weekly review — 30 minutes, every Friday

1. **Pull the numbers** into the sheet (10 min).
2. **Identify the top 2 and bottom 2 posts by profile visits** — not by likes (5 min).
3. **Ask the only question that matters:** what did the top posts have in common? Almost always it will be a specific number, a named failure, or a concrete before/after. Write down the pattern.
4. **Adjust next week's hooks** toward that pattern. The copy is pre-written, but hooks are yours to swap — that's the intended flexibility (5 min).
5. **Check audit requests against the weekly target of 3–6.** Below 2 for two consecutive weeks means the *offer* is not landing, not the content. Fix the offer page before writing more posts (5 min).
6. **Log one insight** into a running doc. By Day 30 this becomes your content strategy for Q4 (5 min).

---

## 6. Decision gates

| Gate | If yes | If no |
|---|---|---|
| **Day 10:** ≥3 audit requests? | Continue as planned | Offer page or CTA is broken. Stop and fix before writing more. |
| **Day 17:** LinkedIn engagement >3%? | Push harder on the winning arc | Your hooks are too abstract. Rewrite openers with specific numbers. |
| **Day 21:** Any AI engine citing you? | Screenshot it — that's your Week 4 hero post | Normal at 3 weeks. Keep publishing answer assets; re-test Day 45. |
| **Day 30:** ≥8 audit requests and ≥3 calls? | Extend to a 90-day program; consider paid amplification of the top 3 posts | Diagnose before repeating: was it reach, offer, or fit? |

---

## 7. The Day 30 deliverable

The campaign's most valuable output is not leads. It's this asset:

> **"How we made ourselves visible to AI answer engines in 30 days — with the before and after."**

Structure it as: baseline prompt results (screenshots) → what we changed (specific, with the answer assets listed) → after results (screenshots) → what moved and what didn't → what we'd do differently.

Publish it as a page on your site, a LinkedIn post, an X thread, and a Dev.to article. Include the failures — the prompts where you *still* don't appear. That honesty is exactly what makes it credible, and it is on-brand with the engineering voice this entire campaign is built on.

Then it becomes the standing sales asset for every $3,500 GEO sprint you sell after this.
