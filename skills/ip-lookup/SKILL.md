---
name: ip-lookup
description: Look up public IP address and geolocation information for any IP or your own
version: 1.0.0
author: OpenIntentOS
tags: [network, utility, geolocation]
requires:
  bins: [curl]
  env: []
tools:
  - name: lookup
    description: Look up geolocation and network info for an IP address (or own public IP if no argument given)
    script: ./lookup.sh
    args:
      - name: ip
        type: string
        required: false
        description: IP address to look up. Omit to look up the machine's own public IP.
---

# IP Lookup

Queries ipinfo.io to retrieve geolocation and network information for any IP address. When called without an argument it returns information about the machine's own public IP.

## Usage

```bash
# Own public IP
./lookup.sh

# Specific IP
./lookup.sh 8.8.8.8
./lookup.sh 1.1.1.1
```

## Output

Returns JSON:

```json
{
  "ip": "8.8.8.8",
  "hostname": "dns.google",
  "city": "Mountain View",
  "region": "California",
  "country": "US",
  "org": "AS15169 Google LLC",
  "timezone": "America/Los_Angeles",
  "loc": "37.3860,-122.0840",
  "postal": "94035"
}
```

## Notes

- Uses ipinfo.io free tier (no API key required for basic lookups).
- `jq` is used for formatting when available; raw JSON is returned otherwise.
- For private/reserved IPs (e.g. 192.168.x.x), ipinfo.io may return limited or no data.
