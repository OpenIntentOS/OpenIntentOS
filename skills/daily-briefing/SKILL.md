---
name: daily-briefing
description: Delivers a personalized morning briefing with tasks, emails, calendar events, news, and system status.
version: "1.0.0"
requires:
  env: []
  bins: []
tags:
  - productivity
  - briefing
  - automation
  - morning
author: OpenIntentOS Contributors
---

# daily-briefing

Delivers a personalized morning briefing with tasks, emails, calendar events, news, and system status.

## Triggers

- **cron**: reads `BRIEFING_TIME` env var (default `07:00`) and runs once per day at that time.
- **manual**: responds to phrases such as "morning briefing", "daily summary", "what's on today", "brief me", "what do I have today".

## Actions

When triggered, perform the following steps in order. Each step is independent — if one fails, include an "unavailable" placeholder and continue.

1. **Fetch calendar events** — list all calendar events for today using the calendar adapter. Format as bullet points with time and title.
2. **Summarize email** — count unread emails and describe the top 3 by subject and sender.
3. **List pending tasks** — retrieve open tasks from memory/task store. Show up to 5 with priority indicators.
4. **Fetch news headlines** — run a web search for "top news today" and extract the top 3 results with title and source.
5. **Check system health** — report OpenIntentOS version, approximate uptime, and memory status (OK / Warning).
6. **Compose and deliver** — format the above as a Markdown briefing using the template below, then send via the configured delivery channel: Telegram if `TELEGRAM_BOT_TOKEN` is set, otherwise display in the web UI chat.

## Output Format

```markdown
# Morning Briefing — {Day}, {Date}

## Calendar
{events or "No events today"}

## Email
{unread_count} unread — Top messages:
{top 3 subjects and senders, or "Inbox clear"}

## Tasks
{pending tasks with priority, or "All clear — no pending tasks"}

## News
{top 3 headlines with sources, or "No headlines available"}

## System
OpenIntentOS v{version} — uptime {duration}, memory OK
```

## Behavior Notes

- Keep the briefing concise. Aim for under 400 words total.
- If `BRIEFING_ENABLED=false` in the environment, skip the cron trigger but still respond to manual triggers.
- For the cron trigger: check `BRIEFING_TIME` env var. Default to `07:00` if unset. The value is in 24-hour `HH:MM` format, local timezone.
- Do not repeat the same briefing within the same calendar day unless explicitly asked.
