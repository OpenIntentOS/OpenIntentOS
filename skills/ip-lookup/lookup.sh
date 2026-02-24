#!/bin/bash
# IP lookup script - returns public IP and geolocation as JSON
curl -s https://ipinfo.io/json 2>/dev/null || echo '{"error": "failed to reach ipinfo.io"}'
