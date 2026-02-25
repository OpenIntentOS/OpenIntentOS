//! Auto-memory system for intelligent conversation and task tracking.
//!
//! This module provides automatic memory management that learns from conversations
//! and tracks completed tasks, user preferences, and interaction patterns.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::error::Result;
use crate::llm::types::Message;

/// Configuration for the auto-memory system.
#[derive(Debug, Clone)]
pub struct AutoMemoryConfig {
    /// Minimum importance threshold for saving memories (0.0-1.0).
    pub min_importance: f64,
    
    /// Maximum number of memories to keep per category.
    pub max_memories_per_category: usize,
    
    /// How often to analyze conversations for memory extraction.
    pub analysis_interval: Duration,
    
    /// Whether to automatically save task completions.
    pub auto_save_tasks: bool,
    
    /// Whether to automatically save user preferences.
    pub auto_save_preferences: bool,
    
    /// Whether to automatically save interaction patterns.
    pub auto_save_patterns: bool,
}

impl Default for AutoMemoryConfig {
    fn default() -> Self {
        Self {
            min_importance: 0.6,
            max_memories_per_category: 100,
            analysis_interval: Duration::from_secs(60),
            auto_save_tasks: true,
            auto_save_preferences: true,
            auto_save_patterns: true,
        }
    }
}

/// Represents different types of memories we can extract.
#[derive(Debug, Clone, PartialEq)]
pub enum MemoryType {
    /// User preferences and settings.
    Preference,
    /// Factual knowledge learned from conversations.
    Knowledge,
    /// Behavioral patterns and interaction styles.
    Pattern,
    /// Skills and capabilities the user has or wants.
    Skill,
    /// Completed tasks and their outcomes.
    Task,
}

impl MemoryType {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryType::Preference => "preference",
            MemoryType::Knowledge => "knowledge", 
            MemoryType::Pattern => "pattern",
            MemoryType::Skill => "skill",
            MemoryType::Task => "task",
        }
    }
}

/// A memory entry to be saved.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub memory_type: MemoryType,
    pub content: String,
    pub importance: f64,
    pub context: HashMap<String, String>,
    pub timestamp: u64,
}

impl MemoryEntry {
    pub fn new(memory_type: MemoryType, content: String, importance: f64) -> Self {
        Self {
            memory_type,
            content,
            importance,
            context: HashMap::new(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }
    
    pub fn with_context(mut self, key: String, value: String) -> Self {
        self.context.insert(key, value);
        self
    }
}

/// Trait for memory storage backends.
#[async_trait::async_trait]
pub trait MemoryStore: Send + Sync {
    async fn save_memory(&self, entry: MemoryEntry) -> Result<u64>;
    async fn search_memories(&self, query: &str, memory_type: Option<MemoryType>, limit: usize) -> Result<Vec<(u64, MemoryEntry)>>;
    async fn get_recent_memories(&self, memory_type: Option<MemoryType>, limit: usize) -> Result<Vec<(u64, MemoryEntry)>>;
    async fn delete_memory(&self, id: u64) -> Result<()>;
}

/// Auto-memory manager that analyzes conversations and extracts important information.
pub struct AutoMemoryManager {
    config: AutoMemoryConfig,
    store: Arc<dyn MemoryStore>,
    conversation_buffer: Arc<Mutex<Vec<Message>>>,
    last_analysis: Arc<Mutex<SystemTime>>,
    user_context: Arc<Mutex<HashMap<String, String>>>,
}

impl AutoMemoryManager {
    pub fn new(config: AutoMemoryConfig, store: Arc<dyn MemoryStore>) -> Self {
        Self {
            config,
            store,
            conversation_buffer: Arc::new(Mutex::new(Vec::new())),
            last_analysis: Arc::new(Mutex::new(SystemTime::now())),
            user_context: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    
    /// Add a message to the conversation buffer for analysis.
    pub async fn add_message(&self, message: Message) {
        let mut buffer = self.conversation_buffer.lock().await;
        buffer.push(message);
        
        // Trigger analysis if enough time has passed
        let should_analyze = {
            let mut last = self.last_analysis.lock().await;
            let now = SystemTime::now();
            if now.duration_since(*last).unwrap_or_default() >= self.config.analysis_interval {
                *last = now;
                true
            } else {
                false
            }
        };
        
        if should_analyze {
            self.analyze_conversation().await;
        }
    }
    
    /// Record a completed task for memory.
    pub async fn record_task_completion(&self, task_description: String, result: String, success: bool) -> Result<()> {
        if !self.config.auto_save_tasks {
            return Ok(());
        }
        
        let importance = if success { 0.8 } else { 0.6 };
        let content = format!(
            "Task: {} | Result: {} | Success: {}", 
            task_description, result, success
        );
        
        let entry = MemoryEntry::new(MemoryType::Task, content, importance)
            .with_context("task_type".to_string(), "completion".to_string())
            .with_context("success".to_string(), success.to_string());
            
        self.store.save_memory(entry).await?;
        Ok(())
    }
    
    /// Record a user preference.
    pub async fn record_preference(&self, preference: String, importance: f64) -> Result<()> {
        if !self.config.auto_save_preferences {
            return Ok(());
        }
        
        let entry = MemoryEntry::new(MemoryType::Preference, preference, importance);
        self.store.save_memory(entry).await?;
        Ok(())
    }
    
    /// Record an interaction pattern.
    pub async fn record_pattern(&self, pattern: String, importance: f64) -> Result<()> {
        if !self.config.auto_save_patterns {
            return Ok(());
        }
        
        let entry = MemoryEntry::new(MemoryType::Pattern, pattern, importance);
        self.store.save_memory(entry).await?;
        Ok(())
    }
    
    /// Analyze the conversation buffer and extract memories.
    async fn analyze_conversation(&self) {
        let messages = {
            let mut buffer = self.conversation_buffer.lock().await;
            if buffer.is_empty() {
                return;
            }
            let messages = buffer.clone();
            buffer.clear();
            messages
        };
        
        // Extract different types of memories from the conversation
        if let Err(e) = self.extract_memories_from_messages(&messages).await {
            tracing::warn!("Failed to extract memories from conversation: {}", e);
        }
    }
    
    /// Extract memories from a set of messages.
    async fn extract_memories_from_messages(&self, messages: &[Message]) -> Result<()> {
        // Look for user preferences
        for message in messages {
            if message.role == crate::llm::Role::User {
                let text = message.content_text();
                
                // Detect preference patterns
                if let Some(preference) = self.extract_preference(&text) {
                    let _ = self.record_preference(preference, 0.7).await;
                }
                
                // Detect patterns in user behavior
                if let Some(pattern) = self.extract_pattern(&text) {
                    let _ = self.record_pattern(pattern, 0.6).await;
                }
            }
        }
        
        Ok(())
    }
    
    /// Extract user preferences from text.
    fn extract_preference(&self, text: &str) -> Option<String> {
        let text_lower = text.to_lowercase();
        
        // Language preference
        if text_lower.contains("我喜欢") || text_lower.contains("我更喜欢") {
            return Some(format!("用户表达了偏好: {}", text.trim()));
        }
        
        if text_lower.contains("i prefer") || text_lower.contains("i like") {
            return Some(format!("User expressed preference: {}", text.trim()));
        }
        
        // Response format preferences
        if text_lower.contains("表格") || text_lower.contains("table") {
            return Some("用户偏好表格格式的回答".to_string());
        }
        
        if text_lower.contains("简短") || text_lower.contains("brief") {
            return Some("用户偏好简短的回答".to_string());
        }
        
        if text_lower.contains("详细") || text_lower.contains("detailed") {
            return Some("用户偏好详细的回答".to_string());
        }
        
        None
    }
    
    /// Extract behavioral patterns from text.
    fn extract_pattern(&self, text: &str) -> Option<String> {
        let text_lower = text.to_lowercase();
        
        // Question patterns
        if text_lower.starts_with("搜索") || text_lower.starts_with("search") {
            return Some("用户经常请求搜索信息".to_string());
        }
        
        if text_lower.starts_with("帮我") || text_lower.starts_with("help me") {
            return Some("用户经常请求帮助完成任务".to_string());
        }
        
        if text_lower.contains("记住") || text_lower.contains("remember") {
            return Some("用户重视记忆和上下文保持".to_string());
        }
        
        // Technical patterns
        if text_lower.contains("代码") || text_lower.contains("code") {
            return Some("用户经常讨论编程相关内容".to_string());
        }
        
        if text_lower.contains("git") || text_lower.contains("github") {
            return Some("用户经常使用Git相关功能".to_string());
        }
        
        None
    }
    
    /// Get relevant memories for a given context.
    pub async fn get_relevant_memories(&self, query: &str, limit: usize) -> Result<Vec<String>> {
        let memories = self.store.search_memories(query, None, limit).await?;
        Ok(memories.into_iter().map(|(_, entry)| entry.content).collect())
    }
    
    /// Get recent memories of a specific type.
    pub async fn get_recent_memories_by_type(&self, memory_type: MemoryType, limit: usize) -> Result<Vec<String>> {
        let memories = self.store.get_recent_memories(Some(memory_type), limit).await?;
        Ok(memories.into_iter().map(|(_, entry)| entry.content).collect())
    }
    
    /// Update user context information.
    pub async fn update_user_context(&self, key: String, value: String) {
        let mut context = self.user_context.lock().await;
        context.insert(key, value);
    }
    
    /// Get user context information.
    pub async fn get_user_context(&self) -> HashMap<String, String> {
        self.user_context.lock().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::Message;
    
    struct MockMemoryStore {
        memories: Arc<Mutex<Vec<(u64, MemoryEntry)>>>,
        next_id: Arc<Mutex<u64>>,
    }
    
    impl MockMemoryStore {
        fn new() -> Self {
            Self {
                memories: Arc::new(Mutex::new(Vec::new())),
                next_id: Arc::new(Mutex::new(1)),
            }
        }
    }
    
    #[async_trait::async_trait]
    impl MemoryStore for MockMemoryStore {
        async fn save_memory(&self, entry: MemoryEntry) -> Result<u64> {
            let mut memories = self.memories.lock().await;
            let mut next_id = self.next_id.lock().await;
            let id = *next_id;
            *next_id += 1;
            memories.push((id, entry));
            Ok(id)
        }
        
        async fn search_memories(&self, query: &str, memory_type: Option<MemoryType>, limit: usize) -> Result<Vec<(u64, MemoryEntry)>> {
            let memories = self.memories.lock().await;
            let filtered: Vec<_> = memories
                .iter()
                .filter(|(_, entry)| {
                    let type_matches = memory_type.map_or(true, |t| entry.memory_type == t);
                    let content_matches = entry.content.to_lowercase().contains(&query.to_lowercase());
                    type_matches && content_matches
                })
                .take(limit)
                .cloned()
                .collect();
            Ok(filtered)
        }
        
        async fn get_recent_memories(&self, memory_type: Option<MemoryType>, limit: usize) -> Result<Vec<(u64, MemoryEntry)>> {
            let memories = self.memories.lock().await;
            let mut filtered: Vec<_> = memories
                .iter()
                .filter(|(_, entry)| memory_type.map_or(true, |t| entry.memory_type == t))
                .cloned()
                .collect();
            filtered.sort_by(|a, b| b.1.timestamp.cmp(&a.1.timestamp));
            filtered.truncate(limit);
            Ok(filtered)
        }
        
        async fn delete_memory(&self, id: u64) -> Result<()> {
            let mut memories = self.memories.lock().await;
            memories.retain(|(mem_id, _)| *mem_id != id);
            Ok(())
        }
    }
    
    #[tokio::test]
    async fn test_auto_memory_preference_extraction() {
        let store = Arc::new(MockMemoryStore::new());
        let config = AutoMemoryConfig::default();
        let manager = AutoMemoryManager::new(config, store.clone());
        
        // Test Chinese preference
        let message = Message::user("我喜欢表格格式的回答");
        manager.add_message(message).await;
        
        // Force analysis
        manager.analyze_conversation().await;
        
        let memories = store.get_recent_memories(Some(MemoryType::Preference), 10).await.unwrap();
        assert!(!memories.is_empty());
    }
    
    #[tokio::test]
    async fn test_task_completion_recording() {
        let store = Arc::new(MockMemoryStore::new());
        let config = AutoMemoryConfig::default();
        let manager = AutoMemoryManager::new(config, store.clone());
        
        manager.record_task_completion(
            "Search for OpenClaw tutorials".to_string(),
            "Found 15 tutorials and guides".to_string(),
            true
        ).await.unwrap();
        
        let memories = store.get_recent_memories(Some(MemoryType::Task), 10).await.unwrap();
        assert_eq!(memories.len(), 1);
        assert!(memories[0].1.content.contains("Search for OpenClaw tutorials"));
    }
    
    #[tokio::test]
    async fn test_pattern_extraction() {
        let store = Arc::new(MockMemoryStore::new());
        let config = AutoMemoryConfig::default();
        let manager = AutoMemoryManager::new(config, store.clone());
        
        let message = Message::user("帮我搜索一些信息");
        manager.add_message(message).await;
        
        manager.analyze_conversation().await;
        
        let memories = store.get_recent_memories(Some(MemoryType::Pattern), 10).await.unwrap();
        assert!(!memories.is_empty());
    }
}