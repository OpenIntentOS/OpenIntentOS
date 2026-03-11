---
name: weather-check
description: Check current weather for any location using wttr.in
version: 1.0.0
author: OpenIntentOS
tags: [weather, utility]
requires:
  bins: [curl]
  env: []
tools:
  - name: weather
    description: Fetch current weather conditions for a location
    script: ./weather.sh
    args:
      - name: location
        type: string
        required: true
        description: City name, region, or location string (e.g. "London", "New York", "Tokyo")
---

# Weather Check

Checks current weather conditions for any location using the wttr.in API.

## Usage

Run the `weather` tool with a location argument:

```bash
./weather.sh "London"
./weather.sh "New York"
./weather.sh "Tokyo"
```

## Output

Returns JSON:

```json
{
  "location": "London, United Kingdom",
  "temp": "15°C / 59°F",
  "description": "Partly cloudy",
  "humidity": "72%",
  "wind": "18 km/h WSW"
}
```

## Notes

- Uses `jq` for JSON parsing when available; falls back to plain text extraction if not installed.
- Location can be a city name, airport code, or coordinates (`48.8,2.3`).
- Data is sourced from wttr.in which aggregates multiple weather services.
