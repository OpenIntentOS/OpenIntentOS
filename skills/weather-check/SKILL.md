---
name: weather-check
description: Check weather information for any city using wttr.in API.
version: 1.0.0
author: OpenIntentOS
tags:
  - weather
  - utility
metadata:
  openclaw:
    requires:
      bins:
        - curl
    emoji: "sun"
    homepage: https://github.com/OpenIntentOS/skills-weather
---

# Weather Check

Use the `web_fetch` tool to check weather at `https://wttr.in/{city}?format=j1`.
