use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use crate::traits::{Adapter, AdapterType, ToolDefinition, AuthRequirement, HealthStatus};
use crate::error::{AdapterError, Result};

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
    client: Arc<Mutex<Option<rumqttc::AsyncClient>>>,
    message_sender: Arc<Mutex<Option<mpsc::UnboundedSender<MqttMessage>>>>,
    subscriptions: Arc<Mutex<HashMap<String, QoS>>>,
}

impl MqttAdapter {
    /// Create a new MQTT adapter with configuration
    pub fn new(config: MqttConfig) -> Self {
        Self {
            config,
            client: Arc::new(Mutex::new(None)),
            message_sender: Arc::new(Mutex::new(None)),
            subscriptions: Arc::new(Mutex::new(HashMap::new())),
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
            
            let tls_config = if let (Some(client_cert), Some(client_key)) = (&self.config.client_cert, &self.config.client_key) {
                let cert = std::fs::read(client_cert)
                    .map_err(|e| AdapterError::ConfigError(format!("Failed to read client cert: {}", e)))?;
                let key = std::fs::read(client_key)
                    .map_err(|e| AdapterError::ConfigError(format!("Failed to read client key: {}", e)))?;

                rumqttc::TlsConfiguration::Simple {
                    ca: ca.into(),
                    alpn: None,
                    client_auth: Some((cert.into(), key.into())),
                }
            } else {
                rumqttc::TlsConfiguration::Simple {
                    ca: ca.into(),
                    alpn: None,
                    client_auth: None,
                }
            };

            mqttoptions.set_transport(rumqttc::Transport::Tls(tls_config));
        }

        let (client, _event_loop) = rumqttc::AsyncClient::new(mqttoptions, 10);
        
        *self.client.lock().await = Some(client.clone());

        // Start message handling task
        let (tx, mut rx) = mpsc::unbounded_channel::<MqttMessage>();
        *self.message_sender.lock().await = Some(tx);

        tokio::spawn(async move {
            while let Some(message) = rx.recv().await {
                let rumqtt_qos = match message.qos {
                    QoS::AtMostOnce => rumqttc::QoS::AtMostOnce,
                    QoS::AtLeastOnce => rumqttc::QoS::AtLeastOnce,
                    QoS::ExactlyOnce => rumqttc::QoS::ExactlyOnce,
                };
                
                if let Err(e) = client.publish(
                    &message.topic,
                    rumqtt_qos,
                    message.retain,
                    message.payload,
                ).await {
                    eprintln!("Failed to publish MQTT message: {}", e);
                }
            }
        });

        Ok(())
    }

    /// Publish a message to a topic
    pub async fn publish(&self, topic: &str, payload: Vec<u8>, qos: QoS, retain: bool) -> Result<()> {
        let sender = self.message_sender.lock().await;
        if let Some(sender) = sender.as_ref() {
            let message = MqttMessage {
                topic: topic.to_string(),
                payload,
                qos,
                retain,
            };
            
            sender.send(message)
                .map_err(|e| AdapterError::ExecutionError(format!("Failed to send message: {}", e)))?;
            
            Ok(())
        } else {
            Err(AdapterError::ConfigError("MQTT client not connected".to_string()))
        }
    }

    /// Subscribe to a topic
    pub async fn subscribe(&mut self, topic: &str, qos: QoS) -> Result<()> {
        let client = self.client.lock().await;
        if let Some(client) = client.as_ref() {
            let rumqtt_qos = match qos {
                QoS::AtMostOnce => rumqttc::QoS::AtMostOnce,
                QoS::AtLeastOnce => rumqttc::QoS::AtLeastOnce,
                QoS::ExactlyOnce => rumqttc::QoS::ExactlyOnce,
            };
            
            client.subscribe(topic, rumqtt_qos).await
                .map_err(|e| AdapterError::ExecutionError(format!("Failed to subscribe: {}", e)))?;
            
            self.subscriptions.lock().await.insert(topic.to_string(), qos);
            Ok(())
        } else {
            Err(AdapterError::ConfigError("MQTT client not connected".to_string()))
        }
    }

    /// Unsubscribe from a topic
    pub async fn unsubscribe(&mut self, topic: &str) -> Result<()> {
        let client = self.client.lock().await;
        if let Some(client) = client.as_ref() {
            client.unsubscribe(topic).await
                .map_err(|e| AdapterError::ExecutionError(format!("Failed to unsubscribe: {}", e)))?;
            
            self.subscriptions.lock().await.remove(topic);
            Ok(())
        } else {
            Err(AdapterError::ConfigError("MQTT client not connected".to_string()))
        }
    }

    /// Get list of active subscriptions
    pub async fn get_subscriptions(&self) -> HashMap<String, QoS> {
        self.subscriptions.lock().await.clone()
    }

    /// Disconnect from MQTT broker
    pub async fn disconnect_mqtt(&mut self) -> Result<()> {
        let client = self.client.lock().await;
        if let Some(client) = client.as_ref() {
            client.disconnect().await
                .map_err(|e| AdapterError::ExecutionError(format!("Failed to disconnect: {}", e)))?;
        }
        
        drop(client);
        *self.client.lock().await = None;
        *self.message_sender.lock().await = None;
        self.subscriptions.lock().await.clear();
        
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
        self.connect_mqtt().await
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.disconnect_mqtt().await
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        let client = self.client.lock().await;
        if client.is_some() {
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
                    .ok_or_else(|| AdapterError::InvalidInput("Missing 'topic' parameter".to_string()))?;
                
                let payload = params["payload"].as_str()
                    .ok_or_else(|| AdapterError::InvalidInput("Missing 'payload' parameter".to_string()))?
                    .as_bytes().to_vec();

                let qos = match params["qos"].as_u64().unwrap_or(0) {
                    0 => QoS::AtMostOnce,
                    1 => QoS::AtLeastOnce,
                    2 => QoS::ExactlyOnce,
                    _ => QoS::AtMostOnce,
                };

                let retain = params["retain"].as_bool().unwrap_or(false);

                self.publish(topic, payload, qos, retain).await?;
                
                Ok(serde_json::json!({
                    "success": true,
                    "topic": topic,
                    "qos": qos as u8,
                    "retain": retain
                }))
            },
            
            "mqtt_subscribe" => {
                let topic = params["topic"].as_str()
                    .ok_or_else(|| AdapterError::InvalidInput("Missing 'topic' parameter".to_string()))?;
                
                let qos = match params["qos"].as_u64().unwrap_or(0) {
                    0 => QoS::AtMostOnce,
                    1 => QoS::AtLeastOnce,
                    2 => QoS::ExactlyOnce,
                    _ => QoS::AtMostOnce,
                };

                let client = self.client.lock().await;
                if let Some(client) = client.as_ref() {
                    let rumqtt_qos = match qos {
                        QoS::AtMostOnce => rumqttc::QoS::AtMostOnce,
                        QoS::AtLeastOnce => rumqttc::QoS::AtLeastOnce,
                        QoS::ExactlyOnce => rumqttc::QoS::ExactlyOnce,
                    };
                    
                    client.subscribe(topic, rumqtt_qos).await
                        .map_err(|e| AdapterError::ExecutionError(format!("Failed to subscribe: {}", e)))?;
                    
                    Ok(serde_json::json!({
                        "success": true,
                        "topic": topic,
                        "qos": qos as u8
                    }))
                } else {
                    Err(AdapterError::ConfigError("MQTT client not connected".to_string()))
                }
            },
            
            "mqtt_unsubscribe" => {
                let topic = params["topic"].as_str()
                    .ok_or_else(|| AdapterError::InvalidInput("Missing 'topic' parameter".to_string()))?;
                
                let client = self.client.lock().await;
                if let Some(client) = client.as_ref() {
                    client.unsubscribe(topic).await
                        .map_err(|e| AdapterError::ExecutionError(format!("Failed to unsubscribe: {}", e)))?;
                    
                    Ok(serde_json::json!({
                        "success": true,
                        "topic": topic
                    }))
                } else {
                    Err(AdapterError::ConfigError("MQTT client not connected".to_string()))
                }
            },
            
            "mqtt_list_subscriptions" => {
                let subscriptions = self.get_subscriptions().await;
                let subscriptions: Vec<serde_json::Value> = subscriptions.iter()
                    .map(|(topic, qos)| serde_json::json!({
                        "topic": topic,
                        "qos": *qos as u8
                    }))
                    .collect();
                
                Ok(serde_json::json!({
                    "subscriptions": subscriptions
                }))
            },
            
            _ => Err(AdapterError::InvalidInput(format!("Unknown tool: {}", name)))
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
        
        assert_eq!(adapter.id(), "mqtt");
        let subscriptions = adapter.get_subscriptions().await;
        assert!(subscriptions.is_empty());
    }

    #[test]
    fn test_qos_serialization() {
        assert_eq!(QoS::AtMostOnce as u8, 0);
        assert_eq!(QoS::AtLeastOnce as u8, 1);
        assert_eq!(QoS::ExactlyOnce as u8, 2);
    }
}