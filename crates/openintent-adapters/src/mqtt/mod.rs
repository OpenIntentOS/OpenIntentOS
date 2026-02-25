use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::traits::{Adapter, AdapterType, ToolDefinition, AuthRequirement, HealthStatus};
use crate::error::Result;

/// MQTT Quality of Service levels
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum QoS {
    AtMostOnce = 0,
    AtLeastOnce = 1,
    ExactlyOnce = 2,
}

/// MQTT message structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MqttMessage {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: QoS,
    pub retain: bool,
}

/// MQTT subscription configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MqttSubscription {
    pub topic: String,
    pub qos: QoS,
}

/// MQTT connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MqttConfig {
    pub broker_url: String,
    pub client_id: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub keep_alive: u16,
    pub clean_session: bool,
    pub ca_cert: Option<String>,
    pub client_cert: Option<String>,
    pub client_key: Option<String>,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            broker_url: "mqtt://localhost:1883".to_string(),
            client_id: Some(format!("openintent-{}", Uuid::new_v4())),
            username: None,
            password: None,
            keep_alive: 60,
            clean_session: true,
            ca_cert: None,
            client_cert: None,
            client_key: None,
        }
    }
}

/// MQTT adapter for IoT device communication
pub struct MqttAdapter {
    config: MqttConfig,
    client: Option<rumqttc::AsyncClient>,
    event_loop: Option<rumqttc::EventLoop>,
    message_sender: Option<mpsc::UnboundedSender<MqttMessage>>,
    subscriptions: HashMap<String, QoS>,
}

impl MqttAdapter {
    /// Create a new MQTT adapter with configuration
    pub fn new(config: MqttConfig) -> Self {
        Self {
            config,
            client: None,
            event_loop: None,
            message_sender: None,
            subscriptions: HashMap::new(),
        }
    }

    /// Connect to MQTT broker
    pub async fn connect_mqtt(&mut self) -> Result<()> {
        let mut mqttoptions = rumqttc::MqttOptions::new(
            self.config.client_id.clone().unwrap_or_else(|| format!("openintent-{}", Uuid::new_v4())),
            &self.config.broker_url,
            1883,
        );

        mqttoptions.set_keep_alive(std::time::Duration::from_secs(self.config.keep_alive as u64));
        mqttoptions.set_clean_session(self.config.clean_session);

        if let (Some(username), Some(password)) = (&self.config.username, &self.config.password) {
            mqttoptions.set_credentials(username, password);
        }

        // TLS configuration if certificates are provided
        if let Some(ca_cert) = &self.config.ca_cert {
            let ca = std::fs::read(ca_cert)
                .map_err(|e| AdapterError::ConfigError(format!("Failed to read CA cert: {}", e)))?;
            
            let mut tls_config = rumqttc::TlsConfiguration::Simple {
                ca: ca.into(),
                alpn: None,
                client_auth: None,
            };

            if let (Some(client_cert), Some(client_key)) = (&self.config.client_cert, &self.config.client_key) {
                let cert = std::fs::read(client_cert)
                    .map_err(|e| AdapterError::ConfigError(format!("Failed to read client cert: {}", e)))?;
                let key = std::fs::read(client_key)
                    .map_err(|e| AdapterError::ConfigError(format!("Failed to read client key: {}", e)))?;

                tls_config = rumqttc::TlsConfiguration::Simple {
                    ca: ca.into(),
                    alpn: None,
                    client_auth: Some(rumqttc::ClientAuth {
                        certs: cert.into(),
                        key: key.into(),
                    }),
                };
            }

            mqttoptions.set_transport(rumqttc::Transport::Tls(tls_config));
        }

        let (client, event_loop) = rumqttc::AsyncClient::new(mqttoptions, 10);
        
        self.client = Some(client);
        self.event_loop = Some(event_loop);

        // Start message handling task
        let (tx, mut rx) = mpsc::unbounded_channel::<MqttMessage>();
        self.message_sender = Some(tx);

        if let Some(client) = &self.client {
            let client_clone = client.clone();
            tokio::spawn(async move {
                while let Some(message) = rx.recv().await {
                    if let Err(e) = client_clone.publish(
                        &message.topic,
                        message.qos as u8,
                        message.retain,
                        message.payload,
                    ).await {
                        eprintln!("Failed to publish MQTT message: {}", e);
                    }
                }
            });
        }

        Ok(())
    }

    /// Publish a message to a topic
    pub async fn publish(&self, topic: &str, payload: Vec<u8>, qos: QoS, retain: bool) -> Result<()> {
        if let Some(sender) = &self.message_sender {
            let message = MqttMessage {
                topic: topic.to_string(),
                payload,
                qos,
                retain,
            };
            
            sender.send(message)
                .map_err(|e| crate::error::AdapterError::ExecutionError(format!("Failed to send message: {}", e)))?;
            
            Ok(())
        } else {
            Err(AdapterError::ConfigError("MQTT client not connected".to_string()))
        }
    }

    /// Subscribe to a topic
    pub async fn subscribe(&mut self, topic: &str, qos: QoS) -> Result<()> {
        if let Some(client) = &self.client {
            client.subscribe(topic, rumqttc::QoS::from(qos as u8)).await
                .map_err(|e| crate::error::AdapterError::ExecutionError(format!("Failed to subscribe: {}", e)))?;
            
            self.subscriptions.insert(topic.to_string(), qos);
            Ok(())
        } else {
            Err(crate::error::AdapterError::ConfigError("MQTT client not connected".to_string()).into())
        }
    }

    /// Unsubscribe from a topic
    pub async fn unsubscribe(&mut self, topic: &str) -> Result<()> {
        if let Some(client) = &self.client {
            client.unsubscribe(topic).await
                .map_err(|e| crate::error::AdapterError::ExecutionError(format!("Failed to unsubscribe: {}", e)))?;
            
            self.subscriptions.remove(topic);
            Ok(())
        } else {
            Err(crate::error::AdapterError::ConfigError("MQTT client not connected".to_string()).into())
        }
    }

    /// Get list of active subscriptions
    pub fn get_subscriptions(&self) -> &HashMap<String, QoS> {
        &self.subscriptions
    }

    /// Disconnect from MQTT broker
    pub async fn disconnect_mqtt(&mut self) -> Result<()> {
        if let Some(client) = &self.client {
            client.disconnect().await
                .map_err(|e| AdapterError::ExecutionError(format!("Failed to disconnect: {}", e)))?;
        }
        
        self.client = None;
        self.event_loop = None;
        self.message_sender = None;
        self.subscriptions.clear();
        
        Ok(())
    }
}

#[async_trait]
impl Adapter for MqttAdapter {
    fn id(&self) -> &str {
        "mqtt"
    }

    fn adapter_type(&self) -> AdapterType {
        AdapterType::System
    }

    async fn connect(&mut self) -> Result<()> {
        let mut mqttoptions = rumqttc::MqttOptions::new(
            self.config.client_id.clone().unwrap_or_else(|| format!("openintent-{}", Uuid::new_v4())),
            &self.config.broker_url,
            1883,
        );

        mqttoptions.set_keep_alive(std::time::Duration::from_secs(self.config.keep_alive as u64));
        mqttoptions.set_clean_session(self.config.clean_session);

        if let (Some(username), Some(password)) = (&self.config.username, &self.config.password) {
            mqttoptions.set_credentials(username, password);
        }

        let (client, event_loop) = rumqttc::AsyncClient::new(mqttoptions, 10);
        
        self.client = Some(client);
        self.event_loop = Some(event_loop);

        // Start message handling task
        let (tx, mut rx) = mpsc::unbounded_channel::<MqttMessage>();
        self.message_sender = Some(tx);

        if let Some(client) = &self.client {
            let client_clone = client.clone();
            tokio::spawn(async move {
                while let Some(message) = rx.recv().await {
                    if let Err(e) = client_clone.publish(
                        &message.topic,
                        rumqttc::QoS::from(message.qos as u8),
                        message.retain,
                        message.payload,
                    ).await {
                        eprintln!("Failed to publish MQTT message: {}", e);
                    }
                }
            });
        }

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        if let Some(client) = &self.client {
            let _ = client.disconnect().await;
        }
        
        self.client = None;
        self.event_loop = None;
        self.message_sender = None;
        self.subscriptions.clear();
        
        Ok(())
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        if self.client.is_some() {
            Ok(HealthStatus::Healthy)
        } else {
            Ok(HealthStatus::Unhealthy)
        }
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "mqtt_publish".to_string(),
                description: "Publish a message to an MQTT topic".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "topic": {
                            "type": "string",
                            "description": "MQTT topic to publish to"
                        },
                        "payload": {
                            "type": "string",
                            "description": "Message payload to publish"
                        },
                        "qos": {
                            "type": "integer",
                            "description": "Quality of Service level (0, 1, or 2)",
                            "default": 0
                        },
                        "retain": {
                            "type": "boolean",
                            "description": "Whether to retain the message",
                            "default": false
                        }
                    },
                    "required": ["topic", "payload"]
                }),
            },
            ToolDefinition {
                name: "mqtt_subscribe".to_string(),
                description: "Subscribe to an MQTT topic".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "topic": {
                            "type": "string",
                            "description": "MQTT topic to subscribe to"
                        },
                        "qos": {
                            "type": "integer",
                            "description": "Quality of Service level (0, 1, or 2)",
                            "default": 0
                        }
                    },
                    "required": ["topic"]
                }),
            },
            ToolDefinition {
                name: "mqtt_unsubscribe".to_string(),
                description: "Unsubscribe from an MQTT topic".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "topic": {
                            "type": "string",
                            "description": "MQTT topic to unsubscribe from"
                        }
                    },
                    "required": ["topic"]
                }),
            },
            ToolDefinition {
                name: "mqtt_list_subscriptions".to_string(),
                description: "List all active MQTT subscriptions".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
        ]
    }

    async fn execute_tool(
        &self,
        name: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        match name {
            "mqtt_publish" => {
                let topic = params["topic"].as_str()
                    .ok_or_else(|| crate::error::AdapterError::InvalidInput("Missing 'topic' parameter".to_string()))?;
                
                let payload = params["payload"].as_str()
                    .ok_or_else(|| crate::error::AdapterError::InvalidInput("Missing 'payload' parameter".to_string()))?
                    .as_bytes().to_vec();

                let qos = match params["qos"].as_u64().unwrap_or(0) {
                    0 => QoS::AtMostOnce,
                    1 => QoS::AtLeastOnce,
                    2 => QoS::ExactlyOnce,
                    _ => QoS::AtMostOnce,
                };

                let retain = params["retain"].as_bool().unwrap_or(false);

                if let Some(sender) = &self.message_sender {
                    let message = MqttMessage {
                        topic: topic.to_string(),
                        payload,
                        qos,
                        retain,
                    };
                    
                    sender.send(message)
                        .map_err(|e| crate::error::AdapterError::ExecutionError(format!("Failed to send message: {}", e)))?;
                    
                    Ok(serde_json::json!({
                        "success": true,
                        "topic": topic,
                        "qos": qos as u8,
                        "retain": retain
                    }))
                } else {
                    Err(crate::error::AdapterError::ConfigError("MQTT client not connected".to_string()).into())
                }
            },
            
            "mqtt_subscribe" => {
                let topic = params["topic"].as_str()
                    .ok_or_else(|| crate::error::AdapterError::InvalidInput("Missing 'topic' parameter".to_string()))?;
                
                let qos = match params["qos"].as_u64().unwrap_or(0) {
                    0 => QoS::AtMostOnce,
                    1 => QoS::AtLeastOnce,
                    2 => QoS::ExactlyOnce,
                    _ => QoS::AtMostOnce,
                };

                if let Some(client) = &self.client {
                    client.subscribe(topic, rumqttc::QoS::from(qos as u8)).await
                        .map_err(|e| crate::error::AdapterError::ExecutionError(format!("Failed to subscribe: {}", e)))?;
                    
                    Ok(serde_json::json!({
                        "success": true,
                        "topic": topic,
                        "qos": qos as u8
                    }))
                } else {
                    Err(crate::error::AdapterError::ConfigError("MQTT client not connected".to_string()).into())
                }
            },
            
            "mqtt_unsubscribe" => {
                let topic = params["topic"].as_str()
                    .ok_or_else(|| crate::error::AdapterError::InvalidInput("Missing 'topic' parameter".to_string()))?;
                
                if let Some(client) = &self.client {
                    client.unsubscribe(topic).await
                        .map_err(|e| crate::error::AdapterError::ExecutionError(format!("Failed to unsubscribe: {}", e)))?;
                    
                    Ok(serde_json::json!({
                        "success": true,
                        "topic": topic
                    }))
                } else {
                    Err(crate::error::AdapterError::ConfigError("MQTT client not connected".to_string()).into())
                }
            },
            
            "mqtt_list_subscriptions" => {
                let subscriptions: Vec<serde_json::Value> = self.subscriptions.iter()
                    .map(|(topic, qos)| serde_json::json!({
                        "topic": topic,
                        "qos": *qos as u8
                    }))
                    .collect();
                
                Ok(serde_json::json!({
                    "subscriptions": subscriptions
                }))
            },
            
            _ => Err(crate::error::AdapterError::InvalidInput(format!("Unknown tool: {}", name)).into())
        }
    }

    fn required_auth(&self) -> Option<AuthRequirement> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mqtt_config_default() {
        let config = MqttConfig::default();
        assert_eq!(config.broker_url, "mqtt://localhost:1883");
        assert!(config.client_id.is_some());
        assert_eq!(config.keep_alive, 60);
        assert!(config.clean_session);
    }

    #[tokio::test]
    async fn test_mqtt_adapter_creation() {
        let config = MqttConfig::default();
        let adapter = MqttAdapter::new(config);
        
        assert_eq!(adapter.name(), "mqtt");
        assert!(adapter.subscriptions.is_empty());
    }

    #[test]
    fn test_qos_serialization() {
        assert_eq!(QoS::AtMostOnce as u8, 0);
        assert_eq!(QoS::AtLeastOnce as u8, 1);
        assert_eq!(QoS::ExactlyOnce as u8, 2);
    }
}