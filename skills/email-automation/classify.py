#!/usr/bin/env python3
"""
Email Classification Script for OpenIntentOS
Analyzes email content and assigns categories based on intelligent heuristics
"""

import json
import sys
import re
from typing import Dict, List, Tuple, Optional
from dataclasses import dataclass
from datetime import datetime, timedelta

@dataclass
class EmailClassification:
    category: str
    priority: str  # high, medium, low
    action_required: bool
    confidence: float
    reasoning: str

class EmailClassifier:
    def __init__(self):
        # Domain patterns for classification
        self.work_domains = {
            'github.com', 'gitlab.com', 'bitbucket.org',
            'slack.com', 'teams.microsoft.com', 'zoom.us',
            'atlassian.com', 'jira.com', 'confluence.com'
        }
        
        self.financial_domains = {
            'paypal.com', 'stripe.com', 'square.com',
            'bank', 'credit', 'invoice', 'billing'
        }
        
        self.social_domains = {
            'facebook.com', 'twitter.com', 'linkedin.com',
            'instagram.com', 'tiktok.com', 'youtube.com'
        }
        
        self.newsletter_domains = {
            'substack.com', 'mailchimp.com', 'constantcontact.com',
            'newsletter', 'digest', 'weekly', 'daily'
        }

    def classify_email(self, subject: str, sender: str, content: str, 
                      received_date: str) -> EmailClassification:
        """Classify an email based on its content and metadata"""
        
        subject_lower = subject.lower()
        sender_lower = sender.lower()
        content_lower = content.lower() if content else ""
        
        # Extract domain from sender
        sender_domain = self._extract_domain(sender)
        
        # Priority indicators
        urgent_keywords = ['urgent', 'asap', 'immediate', 'critical', 'emergency']
        meeting_keywords = ['meeting', 'call', 'zoom', 'calendar', 'invite', 'appointment']
        action_keywords = ['action required', 'please review', 'approval needed', 'response needed']
        
        # Check for urgent/high priority
        is_urgent = any(keyword in subject_lower for keyword in urgent_keywords)
        has_meeting = any(keyword in subject_lower for keyword in meeting_keywords)
        needs_action = any(keyword in subject_lower or keyword in content_lower for keyword in action_keywords)
        
        # Classification logic
        if self._is_spam_or_promotional(subject, sender, content):
            return EmailClassification(
                category="spam_promotional",
                priority="low",
                action_required=False,
                confidence=0.9,
                reasoning="Identified as spam or promotional content"
            )
        
        if self._is_financial(subject, sender, content):
            return EmailClassification(
                category="financial",
                priority="high" if needs_action else "medium",
                action_required=needs_action,
                confidence=0.85,
                reasoning="Financial or billing related email"
            )
        
        if self._is_work_related(subject, sender, content):
            priority = "high" if (is_urgent or needs_action) else "medium"
            return EmailClassification(
                category="work",
                priority=priority,
                action_required=needs_action or has_meeting,
                confidence=0.8,
                reasoning="Work or professional related email"
            )
        
        if self._is_newsletter(subject, sender, content):
            return EmailClassification(
                category="newsletter",
                priority="low",
                action_required=False,
                confidence=0.75,
                reasoning="Newsletter or digest email"
            )
        
        if self._is_social(subject, sender, content):
            return EmailClassification(
                category="social",
                priority="low",
                action_required=False,
                confidence=0.7,
                reasoning="Social media notification"
            )
        
        # Default: personal email
        priority = "high" if is_urgent else "medium"
        return EmailClassification(
            category="personal",
            priority=priority,
            action_required=needs_action,
            confidence=0.6,
            reasoning="Personal or unclassified email"
        )

    def _extract_domain(self, email: str) -> str:
        """Extract domain from email address"""
        try:
            return email.split('@')[1].lower()
        except (IndexError, AttributeError):
            return ""

    def _is_spam_or_promotional(self, subject: str, sender: str, content: str) -> bool:
        """Detect spam or promotional emails"""
        spam_indicators = [
            'unsubscribe', 'promotional', 'offer', 'deal', 'sale', 'discount',
            'free', 'winner', 'congratulations', 'claim', 'limited time',
            'act now', 'click here', 'buy now', 'special offer'
        ]
        
        subject_lower = subject.lower()
        content_lower = content.lower() if content else ""
        
        # Check for multiple spam indicators
        spam_count = sum(1 for indicator in spam_indicators 
                        if indicator in subject_lower or indicator in content_lower)
        
        # Promotional email patterns
        has_unsubscribe = 'unsubscribe' in content_lower
        has_excessive_caps = len([c for c in subject if c.isupper()]) > len(subject) * 0.3
        
        return spam_count >= 2 or has_unsubscribe or has_excessive_caps

    def _is_financial(self, subject: str, sender: str, content: str) -> bool:
        """Detect financial/billing emails"""
        financial_keywords = [
            'invoice', 'payment', 'bill', 'receipt', 'transaction',
            'bank', 'credit', 'debit', 'paypal', 'stripe', 'charge',
            'refund', 'subscription', 'renewal', 'statement'
        ]
        
        domain = self._extract_domain(sender)
        subject_lower = subject.lower()
        
        return (any(keyword in subject_lower for keyword in financial_keywords) or
                any(fin_domain in domain for fin_domain in self.financial_domains))

    def _is_work_related(self, subject: str, sender: str, content: str) -> bool:
        """Detect work-related emails"""
        work_keywords = [
            'project', 'meeting', 'deadline', 'review', 'approval',
            'team', 'client', 'proposal', 'contract', 'report'
        ]
        
        domain = self._extract_domain(sender)
        subject_lower = subject.lower()
        
        return (any(keyword in subject_lower for keyword in work_keywords) or
                any(work_domain in domain for work_domain in self.work_domains))

    def _is_newsletter(self, subject: str, sender: str, content: str) -> bool:
        """Detect newsletter emails"""
        newsletter_keywords = [
            'newsletter', 'digest', 'weekly', 'daily', 'update',
            'roundup', 'summary', 'edition', 'issue'
        ]
        
        domain = self._extract_domain(sender)
        subject_lower = subject.lower()
        
        return (any(keyword in subject_lower for keyword in newsletter_keywords) or
                any(news_domain in domain for news_domain in self.newsletter_domains))

    def _is_social(self, subject: str, sender: str, content: str) -> bool:
        """Detect social media emails"""
        social_keywords = [
            'notification', 'mentioned', 'tagged', 'liked', 'commented',
            'followed', 'friend request', 'message', 'post'
        ]
        
        domain = self._extract_domain(sender)
        subject_lower = subject.lower()
        
        return (any(keyword in subject_lower for keyword in social_keywords) or
                any(social_domain in domain for social_domain in self.social_domains))

def main():
    """Main function to process email classification request"""
    try:
        # Read input from stdin
        input_data = json.loads(sys.stdin.read())
        
        # Extract email data
        subject = input_data.get('subject', '')
        sender = input_data.get('sender', '')
        content = input_data.get('content', '')
        received_date = input_data.get('received_date', '')
        
        # Initialize classifier and classify email
        classifier = EmailClassifier()
        classification = classifier.classify_email(subject, sender, content, received_date)
        
        # Output classification result
        result = {
            'category': classification.category,
            'priority': classification.priority,
            'action_required': classification.action_required,
            'confidence': classification.confidence,
            'reasoning': classification.reasoning,
            'timestamp': datetime.now().isoformat()
        }
        
        print(json.dumps(result, indent=2))
        
    except Exception as e:
        error_result = {
            'error': str(e),
            'category': 'unknown',
            'priority': 'medium',
            'action_required': False,
            'confidence': 0.0,
            'reasoning': f'Classification failed: {str(e)}'
        }
        print(json.dumps(error_result, indent=2))
        sys.exit(1)

if __name__ == "__main__":
    main()