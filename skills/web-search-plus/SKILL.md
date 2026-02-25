---
name: web-search-plus
description: Advanced web search with multiple search engines and intelligent result aggregation
version: 1.0.0
author: OpenIntentOS
tags: [search, web, research, utility]
requires:
  env: []
  bins: []
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