#!/usr/bin/env python3
"""
Batch Email Processing Script
Automatically processes emails in bulk with intelligent categorization and actions.
"""

import json
import sys
import yaml
import re
from datetime import datetime, timedelta
from typing import Dict, List, Any, Optional
from dataclasses import dataclass
from pathlib import Path

@dataclass
class EmailRule:
    """Email processing rule configuration"""
    name: str
    conditions: Dict[str, Any]
    actions: List[str]
    priority: int = 5

@dataclass
class ProcessingResult:
    """Result of batch email processing"""
    total_processed: int = 0
    categorized: int = 0
    archived: int = 0
    deleted: int = 0
    errors: List[str] = None
    
    def __post_init__(self):
        if self.errors is None:
            self.errors = []

class EmailBatchProcessor:
    """Intelligent batch email processor"""
    
    def __init__(self, config_path: str = "config.yaml"):
        self.config = self.load_config(config_path)
        self.rules = self.load_rules()
        self.stats = ProcessingResult()
        
    def load_config(self, config_path: str) -> Dict[str, Any]:
        """Load processing configuration"""
        try:
            with open(config_path, 'r') as f:
                return yaml.safe_load(f)
        except FileNotFoundError:
            return self.get_default_config()
    
    def get_default_config(self) -> Dict[str, Any]:
        """Default configuration if config file not found"""
        return {
            'batch_processing': {
                'max_emails_per_batch': 100,
                'auto_archive_days': 30,
                'spam_threshold': 0.8,
                'categories': {
                    'work': ['work', 'business', 'meeting', 'project'],
                    'personal': ['family', 'friend', 'personal'],
                    'newsletters': ['newsletter', 'unsubscribe', 'marketing'],
                    'notifications': ['notification', 'alert', 'system'],
                    'spam': ['spam', 'phishing', 'scam']
                },
                'auto_actions': {
                    'archive_old': True,
                    'delete_spam': True,
                    'categorize_emails': True,
                    'mark_important': True
                }
            }
        }
    
    def load_rules(self) -> List[EmailRule]:
        """Load email processing rules"""
        rules_config = self.config.get('batch_processing', {}).get('rules', [])
        rules = []
        
        # Default rules
        default_rules = [
            EmailRule(
                name="Archive Old Emails",
                conditions={"age_days": 30, "folder": "inbox"},
                actions=["archive"],
                priority=1
            ),
            EmailRule(
                name="Delete Spam",
                conditions={"spam_score": 0.8},
                actions=["delete"],
                priority=10
            ),
            EmailRule(
                name="Categorize Newsletters",
                conditions={"contains": ["unsubscribe", "newsletter"]},
                actions=["categorize:newsletters", "archive"],
                priority=5
            ),
            EmailRule(
                name="Mark Work Emails Important",
                conditions={"from_domain": ["company.com"], "keywords": ["urgent", "important"]},
                actions=["mark_important", "categorize:work"],
                priority=8
            )
        ]
        
        rules.extend(default_rules)
        
        # Load custom rules from config
        for rule_config in rules_config:
            rule = EmailRule(
                name=rule_config.get('name', 'Custom Rule'),
                conditions=rule_config.get('conditions', {}),
                actions=rule_config.get('actions', []),
                priority=rule_config.get('priority', 5)
            )
            rules.append(rule)
        
        # Sort by priority (higher priority first)
        rules.sort(key=lambda r: r.priority, reverse=True)
        return rules
    
    def process_batch(self, emails: List[Dict[str, Any]]) -> ProcessingResult:
        """Process a batch of emails"""
        max_batch = self.config.get('batch_processing', {}).get('max_emails_per_batch', 100)
        emails_to_process = emails[:max_batch]
        
        for email in emails_to_process:
            try:
                self.process_single_email(email)
                self.stats.total_processed += 1
            except Exception as e:
                self.stats.errors.append(f"Error processing email {email.get('id', 'unknown')}: {str(e)}")
        
        return self.stats
    
    def process_single_email(self, email: Dict[str, Any]) -> None:
        """Process a single email against all rules"""
        applied_actions = set()
        
        for rule in self.rules:
            if self.matches_conditions(email, rule.conditions):
                for action in rule.actions:
                    if action not in applied_actions:
                        self.apply_action(email, action)
                        applied_actions.add(action)
    
    def matches_conditions(self, email: Dict[str, Any], conditions: Dict[str, Any]) -> bool:
        """Check if email matches rule conditions"""
        for condition, value in conditions.items():
            if not self.check_condition(email, condition, value):\n                return False\n        return True\n    \n    def check_condition(self, email: Dict[str, Any], condition: str, value: Any) -> bool:\n        \"\"\"Check a single condition against an email\"\"\"\n        if condition == \"age_days\":\n            email_date = self.parse_email_date(email.get('date', ''))\n            if email_date:\n                age = (datetime.now() - email_date).days\n                return age >= value\n        \n        elif condition == \"folder\":\n            return email.get('folder', '').lower() == value.lower()\n        \n        elif condition == \"spam_score\":\n            return email.get('spam_score', 0) >= value\n        \n        elif condition == \"contains\":\n            content = f\"{email.get('subject', '')} {email.get('body', '')}\".lower()\n            if isinstance(value, list):\n                return any(keyword.lower() in content for keyword in value)\n            else:\n                return value.lower() in content\n        \n        elif condition == \"from_domain\":\n            email_from = email.get('from', '')\n            if isinstance(value, list):\n                return any(domain in email_from for domain in value)\n            else:\n                return value in email_from\n        \n        elif condition == \"keywords\":\n            content = f\"{email.get('subject', '')} {email.get('body', '')}\".lower()\n            if isinstance(value, list):\n                return any(keyword.lower() in content for keyword in value)\n            else:\n                return value.lower() in content\n        \n        return False\n    \n    def apply_action(self, email: Dict[str, Any], action: str) -> None:\n        \"\"\"Apply an action to an email\"\"\"\n        if action == \"archive\":\n            email['action'] = 'archive'\n            self.stats.archived += 1\n        \n        elif action == \"delete\":\n            email['action'] = 'delete'\n            self.stats.deleted += 1\n        \n        elif action.startswith(\"categorize:\"):\n            category = action.split(':', 1)[1]\n            email['category'] = category\n            self.stats.categorized += 1\n        \n        elif action == \"mark_important\":\n            email['important'] = True\n        \n        elif action == \"mark_read\":\n            email['read'] = True\n        \n        elif action == \"mark_unread\":\n            email['read'] = False\n    \n    def parse_email_date(self, date_str: str) -> Optional[datetime]:\n        \"\"\"Parse email date string to datetime object\"\"\"\n        try:\n            # Try common email date formats\n            formats = [\n                \"%a, %d %b %Y %H:%M:%S %z\",\n                \"%d %b %Y %H:%M:%S %z\",\n                \"%Y-%m-%d %H:%M:%S\",\n                \"%Y-%m-%dT%H:%M:%S.%fZ\",\n            ]\n            \n            for fmt in formats:\n                try:\n                    return datetime.strptime(date_str, fmt)\n                except ValueError:\n                    continue\n            \n            return None\n        except Exception:\n            return None\n    \n    def generate_report(self) -> Dict[str, Any]:\n        \"\"\"Generate processing report\"\"\"\n        return {\n            'timestamp': datetime.now().isoformat(),\n            'summary': {\n                'total_processed': self.stats.total_processed,\n                'categorized': self.stats.categorized,\n                'archived': self.stats.archived,\n                'deleted': self.stats.deleted,\n                'errors': len(self.stats.errors)\n            },\n            'details': {\n                'rules_applied': len(self.rules),\n                'processing_time': 'N/A',  # Would be calculated in real implementation\n                'errors': self.stats.errors\n            },\n            'recommendations': self.generate_recommendations()\n        }\n    \n    def generate_recommendations(self) -> List[str]:\n        \"\"\"Generate processing recommendations\"\"\"\n        recommendations = []\n        \n        if self.stats.deleted > self.stats.total_processed * 0.3:\n            recommendations.append(\"High spam detection rate - consider adjusting spam filters\")\n        \n        if self.stats.archived > self.stats.total_processed * 0.8:\n            recommendations.append(\"Many emails archived - consider reviewing inbox management\")\n        \n        if len(self.stats.errors) > 0:\n            recommendations.append(f\"Processing errors occurred ({len(self.stats.errors)}) - check error log\")\n        \n        if self.stats.categorized < self.stats.total_processed * 0.1:\n            recommendations.append(\"Low categorization rate - consider adding more categorization rules\")\n        \n        return recommendations\n\ndef main():\n    \"\"\"Main entry point for batch processing\"\"\"\n    if len(sys.argv) < 2:\n        print(\"Usage: python batch_process.py <emails_json_file>\")\n        sys.exit(1)\n    \n    emails_file = sys.argv[1]\n    \n    try:\n        # Load emails from JSON file\n        with open(emails_file, 'r') as f:\n            emails = json.load(f)\n        \n        # Initialize processor\n        processor = EmailBatchProcessor()\n        \n        # Process emails\n        print(f\"Processing {len(emails)} emails...\")\n        result = processor.process_batch(emails)\n        \n        # Generate and save report\n        report = processor.generate_report()\n        \n        # Output results\n        print(\"\\n=== BATCH PROCESSING COMPLETE ===\")\n        print(f\"Total Processed: {result.total_processed}\")\n        print(f\"Categorized: {result.categorized}\")\n        print(f\"Archived: {result.archived}\")\n        print(f\"Deleted: {result.deleted}\")\n        print(f\"Errors: {len(result.errors)}\")\n        \n        if result.errors:\n            print(\"\\nErrors:\")\n            for error in result.errors:\n                print(f\"  - {error}\")\n        \n        # Save report\n        report_file = f\"email_processing_report_{datetime.now().strftime('%Y%m%d_%H%M%S')}.json\"\n        with open(report_file, 'w') as f:\n            json.dump(report, f, indent=2)\n        \n        print(f\"\\nDetailed report saved to: {report_file}\")\n        \n        # Save processed emails\n        processed_file = f\"processed_emails_{datetime.now().strftime('%Y%m%d_%H%M%S')}.json\"\n        with open(processed_file, 'w') as f:\n            json.dump(emails, f, indent=2)\n        \n        print(f\"Processed emails saved to: {processed_file}\")\n        \n    except FileNotFoundError:\n        print(f\"Error: Emails file '{emails_file}' not found\")\n        sys.exit(1)\n    except json.JSONDecodeError:\n        print(f\"Error: Invalid JSON in '{emails_file}'\")\n        sys.exit(1)\n    except Exception as e:\n        print(f\"Error: {str(e)}\")\n        sys.exit(1)\n\nif __name__ == \"__main__\":\n    main()