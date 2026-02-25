---
name: email-automation
description: Intelligent email management with auto-classification, bulk processing, and smart replies
version: 1.0.0
author: OpenIntentOS
tags: [email, automation, productivity, ai]
requires:
  env: []
  bins: []
  config: []
---

# Email Automation Skill

This skill provides intelligent email management capabilities using OpenIntentOS's built-in email tools.

## Features

### 📧 **Email Classification**
Automatically categorize emails based on:
- Sender patterns (domains, known contacts)
- Subject keywords and patterns  
- Content analysis
- Attachment types

### 🧹 **Bulk Email Processing**
- Process large volumes of unread emails
- Auto-delete spam and promotional emails
- Archive old newsletters and notifications
- Flag important emails for review

### 📊 **Daily Email Summary**
Generate intelligent summaries including:
- Important emails requiring action
- Meeting invitations and calendar updates
- Financial notifications and receipts
- Personal vs work email breakdown

### 🤖 **Smart Auto-Reply**
- Template-based responses for common inquiries
- Out-of-office message management
- Auto-acknowledge receipt of important emails
- Escalation to human review when needed

## Usage

Use the existing OpenIntentOS email tools (`email_list_inbox`, `email_read`, `email_send`, `email_search`) to:

1. **Classify emails**: "Classify my unread emails and show me the important ones"
2. **Bulk cleanup**: "Clean up my inbox by archiving old newsletters and deleting spam"
3. **Daily summary**: "Generate my daily email summary"
4. **Auto-reply setup**: "Set up auto-replies for customer inquiries"

## Configuration

The skill uses intelligent heuristics and can be customized through conversation:
- Define your important contacts and domains
- Set up custom classification rules
- Configure auto-reply templates
- Adjust processing thresholds

## Examples

**Email Classification:**
```
User: "Classify my last 50 emails and show me what needs my attention"
→ Uses email_list_inbox + email_read to analyze and categorize emails
→ Returns prioritized list with categories and action recommendations
```

**Bulk Cleanup:**
```
User: "Clean up emails older than 30 days from newsletters and promotions"  
→ Uses email_search to find old promotional emails
→ Bulk archives or deletes based on sender patterns
→ Reports cleanup statistics
```

**Daily Summary:**
```
User: "What important emails did I receive today?"
→ Uses email_search with date filters
→ Analyzes content for importance signals
→ Generates structured summary with action items
```

This skill leverages OpenIntentOS's existing email infrastructure while adding intelligent automation and AI-powered analysis on top.