---
name: web-search-plus
description: Advanced web search with multiple search engines and intelligent result aggregation
version: 1.0.0
author: OpenIntentOS
tags: [search, web, research, utility]
requires:
  env: []
  bins: [curl, python3]
tools:
  - name: web_search
    description: Search the web using DuckDuckGo and return top results as structured JSON
    script: ./web_search.sh
    args:
      - name: query
        type: string
        required: true
        description: Search query string
---

# Web Search Plus

A comprehensive web search skill that provides intelligent search across multiple search engines with result aggregation and analysis.

## Features

- **Multi-engine search**: Query multiple search engines simultaneously
- **Intelligent aggregation**: Combine and deduplicate results from different sources
- **Deep content analysis**: Automatically fetch and analyze full page content
- **Search result ranking**: Score and rank results by relevance and quality
- **Structured output**: Present results in well-organized tables and categories

## Usage

Use the existing `web_research` tool for comprehensive search with automatic content fetching:

```
web_research("your search query")
```

For quick URL lookups or simple searches, use:

```
web_search("your query")
```

## Search Strategies

1. **Comprehensive Research**: Use `web_research` with multiple query variations
2. **Cross-reference**: Search in both English and Chinese for broader coverage
3. **Deep Analysis**: Automatically fetch full page content for thorough understanding
4. **Result Synthesis**: Combine information from multiple sources into structured summaries

## Best Practices

- Use specific, targeted queries for better results
- Try multiple query formulations for comprehensive coverage
- Leverage both `web_research` and `web_search` based on the task
- Present results in structured tables with clear categories
- Include source URLs for verification

This skill enhances your web search capabilities by providing intelligent query strategies and result presentation guidelines.

## Script Tool

The `web_search.sh` script queries the DuckDuckGo Instant Answer API and returns structured JSON.

```bash
./web_search.sh "OpenIntentOS automation"
./web_search.sh "Rust async runtime comparison"
```

### Output

```json
{
  "query": "OpenIntentOS automation",
  "abstract": {
    "title": "...",
    "text": "...",
    "url": "https://...",
    "source": "..."
  },
  "answer": "",
  "results": [
    {"title": "...", "url": "https://...", "snippet": "..."},
    ...
  ]
}
```

- Up to 5 related results are returned from `RelatedTopics`.
- When `jq` is not installed, a simplified flat JSON is returned.
- No API key required for the DuckDuckGo Instant Answer API.