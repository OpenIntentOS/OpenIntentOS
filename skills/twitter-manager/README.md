# Twitter Manager Skill

Automated Twitter/X account management for @OpenIntentOS with daily posting, content rotation, mention replies, and approval queue.

## Features

### 📅 **Daily Posting Schedule**
- **3 tweets per day** (morning, afternoon, evening)
- **7 content formats** rotating weekly:
  1. **Tech Tips** - AI/automation best practices
  2. **Project Updates** - OpenIntentOS development news
  3. **User Stories** - How people use OpenIntentOS
  4. **Code Snippets** - Useful automation examples
  5. **Industry News** - AI/automation trends
  6. **Q&A** - Answer common questions
  7. **Community Spotlight** - Feature users/contributors

### 🤖 **Auto-Reply System**
- **Mention monitoring**: Track @OpenIntentOS mentions
- **Smart replies**: Categorized responses based on content
- **Escalation**: Flag complex questions for manual review
- **Response templates**: Pre-approved reply templates

### 📋 **Approval Queue**
- **Content review**: All tweets go through approval queue
- **Telegram integration**: Send drafts to Telegram for approval
- **Scheduling**: Approved tweets scheduled automatically
- **Queue management**: View, edit, approve/reject drafts

### 🔄 **Content Pipeline**
- **Source material**: GitHub commits, blog posts, user feedback
- **Content generation**: AI-assisted tweet creation
- **Format optimization**: Character count, hashtags, media
- **Performance tracking**: Engagement metrics analysis

## Configuration

### Environment Variables
```bash
# Twitter/X API Credentials
TWITTER_API_KEY="your_api_key"
TWITTER_API_SECRET="your_api_secret"
TWITTER_ACCESS_TOKEN="your_access_token"
TWITTER_ACCESS_SECRET="your_access_secret"
TWITTER_BEARER_TOKEN="your_bearer_token"

# Telegram for approvals
TELEGRAM_BOT_TOKEN="your_telegram_bot_token"
TELEGRAM_APPROVAL_CHAT_ID="approval_chat_id"

# Posting schedule
TWITTER_POST_TIMES="09:00,14:00,19:00"
TWITTER_TIMEZONE="UTC+8"
```

### Content Format Definitions
Each format has specific templates and hashtags:

| Format | Hashtags | Character Limit | Media |
|--------|----------|-----------------|-------|
| Tech Tips | `#AI #Automation #TechTips` | 240 | Optional |
| Project Updates | `#OpenIntentOS #OpenSource #Rust` | 280 | Screenshots |
| User Stories | `#UserStory #Automation #Productivity` | 260 | User photos |
| Code Snippets | `#Python #Rust #CodeSnippet` | 220 | Code images |
| Industry News | `#AI #TechNews #Innovation` | 250 | Article links |
| Q&A | `#QandA #AI #Help` | 240 | None |
| Community Spotlight | `#Community #OpenSource #Contributors` | 270 | Profile photos |

## Usage

### Manual Commands
```bash
# Post a tweet immediately
python twitter_manager.py --post "Hello Twitter!"

# Generate content for today
python twitter_manager.py --generate

# Check mentions and reply
python twitter_manager.py --check-mentions

# Show approval queue
python twitter_manager.py --queue

# Approve a tweet
python twitter_manager.py --approve TWEET_ID
```

### Automated Schedule
The skill runs automatically via cron:
- **09:00** - Morning tweet + check mentions
- **14:00** - Afternoon tweet + queue processing
- **19:00** - Evening tweet + daily summary
- **Every 15 minutes** - Check mentions for replies

## Approval Workflow

1. **Content Generation** → AI creates tweet drafts
2. **Queue Addition** → Drafts added to approval queue
3. **Telegram Notification** → Sent to approval chat
4. **Human Review** → Approve/reject via Telegram
5. **Scheduling** → Approved tweets scheduled
6. **Posting** → Posted at scheduled times

## Response Categories

### Auto-Reply Templates
- **Greeting**: "Thanks for mentioning @OpenIntentOS!"
- **Question**: "Great question! Let me help with that..."
- **Bug Report**: "Thanks for reporting! We'll look into it."
- **Feature Request**: "Interesting idea! Added to our roadmap."
- **Thank You**: "Glad you're enjoying OpenIntentOS! 😊"

### Escalation Triggers
- Complex technical questions
- Security concerns
- Partnership inquiries
- Negative feedback requiring human touch

## Monitoring & Analytics

### Metrics Tracked
- **Engagement**: Likes, retweets, replies
- **Growth**: Followers gained/lost
- **Response Time**: Time to reply to mentions
- **Queue Stats**: Approval/rejection rates

### Daily Reports
Sent via Telegram:
- Posts published
- Mentions replied
- Queue status
- Engagement highlights

## Integration Points

### With OpenIntentOS
- **GitHub**: Use commit messages for project updates
- **Email**: Convert important emails to tweet content
- **Calendar**: Announce events/webinars
- **Skills**: Cross-promote other skills

### External Services
- **Buffer/Hootsuite**: Alternative scheduling
- **Analytics**: Google Analytics for link tracking
- **CRM**: Track interactions with potential users

## Troubleshooting

### Common Issues
- **API Limits**: Respect Twitter's rate limits
- **Authentication**: Token expiration handling
- **Content Rejection**: Duplicate content detection
- **Queue Stalls**: Manual intervention triggers

### Recovery Procedures
1. Check API status
2. Verify credentials
3. Clear stuck queue items
4. Manual posting as fallback

## Best Practices

1. **Consistency**: Stick to posting schedule
2. **Quality**: All tweets provide value
3. **Engagement**: Always reply to mentions
4. **Transparency**: Be clear about automation
5. **Compliance**: Follow Twitter/X terms of service

## Development

### Adding New Content Formats
1. Add format to `CONTENT_FORMATS` in config
2. Create template file in `templates/`
3. Update rotation schedule
4. Test with `--generate --test`

### Customizing Replies
1. Edit `response_templates.json`
2. Add new categories as needed
3. Update escalation logic
4. Test with sample mentions

This skill transforms @OpenIntentOS into an active, engaging Twitter presence while maintaining quality control through the approval queue system.