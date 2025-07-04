use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use tracing::debug;
use tracing::error;
use tracing::info;
use tracing::instrument;

use crate::error::NetworkError;
use crate::error::ResultMapError;
use crate::FcmError;
use crate::SharedTokenManager;

/// A wrapper for Firebase Cloud Messaging (FCM) notifications.
pub struct FcmNotification {
    pub title: String,
    pub body: String,
}

/// APNS-specific options for FCM messages
#[derive(Debug, Clone, Serialize)]
pub struct ApnsConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fcm_options: Option<ApnsFcmOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_activity_token: Option<String>,
}

/// FCM options for APNS
#[derive(Debug, Clone, Serialize)]
pub struct ApnsFcmOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analytics_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

impl ApnsConfig {
    /// Create a new ApnsConfig with default values
    pub fn new() -> Self {
        Self {
            headers: None,
            payload: None,
            fcm_options: None,
            live_activity_token: None,
        }
    }

    /// Create an ApnsConfig for silent push notifications
    pub fn silent_push() -> Self {
        Self {
            headers: None,
            payload: Some(json!({
                "aps": {
                    "content-available": 1
                }
            })),
            fcm_options: None,
            live_activity_token: None,
        }
    }

    /// Create an ApnsConfig with custom APS payload
    pub fn with_aps_payload(aps_payload: Value) -> Self {
        Self {
            headers: None,
            payload: Some(json!({
                "aps": aps_payload
            })),
            fcm_options: None,
            live_activity_token: None,
        }
    }

    /// Set APNS headers (like apns-priority, apns-expiration)
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = Some(headers);
        self
    }

    /// Set FCM options for APNS
    pub fn with_fcm_options(mut self, fcm_options: ApnsFcmOptions) -> Self {
        self.fcm_options = Some(fcm_options);
        self
    }

    /// Set live activity token
    pub fn with_live_activity_token(mut self, token: String) -> Self {
        self.live_activity_token = Some(token);
        self
    }
}

/// Sends a Firebase Cloud Messaging (FCM) message.
///
/// This function sends an FCM message to the device with the provided device
/// token. You can provide either a data payload or a notification payload, or
/// both. It uses the provided `SharedTokenManager` to handle OAuth tokens.
///
/// # Arguments
///
/// * `device_token` - The device token to send the notification to.
/// * `notification` - An optional `FcmNotification` containing the title and
///   body of the notification.
/// * `data_payload` - Optional data represented as a Map. This can be any type
///   that implements the `Serialize` trait.
/// * `apns_config` - Optional APNS-specific configuration for iOS devices.
/// * `token_manager` - A `SharedTokenManager` to handle OAuth tokens.
/// * `project_id` - The ID of the Firebase project, where the device token is
///   registered.
///
/// # Errors
///
/// This function will return an error if the FCM message could not be sent.
///
/// # Example
///
/// ```rust no_run
/// use std::fs::File;
///
/// use oauth_fcm::{create_shared_token_manager, send_fcm_message, SharedTokenManager, ApnsConfig};
///
/// # tokio_test::block_on(async {
/// let device_token = "device_token";
/// let data = serde_json::json!({
///    "key": "value"
/// });
/// let notification = oauth_fcm::FcmNotification {
///    title: "Test Title".to_string(),
///   body: "Test Body".to_string(),
/// };
/// let apns_config = Some(ApnsConfig::silent_push());
/// let token_manager = create_shared_token_manager(File::open("path_to_google_credentials.json").expect("Failed to open file")).expect("Failed to create SharedTokenManager");
/// let project_id = "project_id";
/// send_fcm_message(device_token, Some(notification), Some(data), apns_config, &token_manager, project_id)
///     .await
///     .expect("Error while sending FCM message");
///
/// # });
/// ```
#[instrument(
    level = "info",
    skip(data_payload, notification, apns_config, token_manager)
)]
pub async fn send_fcm_message<T: Serialize>(
    device_token: &str,
    notification: Option<FcmNotification>,
    data_payload: Option<T>,
    apns_config: Option<ApnsConfig>,
    token_manager: &SharedTokenManager,
    project_id: &str,
) -> Result<(), FcmError> {
    info!("Sending FCM message to device: {}", device_token);
    let url = format!("https://fcm.googleapis.com/v1/projects/{project_id}/messages:send");

    send_fcm_message_with_url(
        device_token,
        notification,
        data_payload,
        apns_config,
        token_manager,
        &url,
    )
    .await
}

/// Sends a Firebase Cloud Messaging (FCM) message to a specific URL.
///
/// This function behaves exactly as `send_fcm`, but allows specifying a custom
/// FCM URL.
///
/// Normally, you would use `send_fcm` instead of this function. This is only
/// useful for testing, such as for mocking the FCM URL.
#[instrument(
    level = "debug",
    skip(data_payload, notification, apns_config, token_manager)
)]
pub async fn send_fcm_message_with_url<T: Serialize>(
    device_token: &str,
    notification: Option<FcmNotification>,
    data_payload: Option<T>,
    apns_config: Option<ApnsConfig>,
    token_manager: &SharedTokenManager,
    fcm_url: &str,
) -> Result<(), FcmError> {
    let access_token = {
        let mut token_manager_guard = token_manager.lock().await;
        token_manager_guard.get_token().await?
    };

    let client = reqwest::Client::new();

    let payload = create_payload(device_token, notification, data_payload, apns_config)?;

    debug!("Requesting access token");

    let res = client
        .post(fcm_url)
        .bearer_auth(access_token)
        .json(&payload)
        .send()
        .await
        .map_err(NetworkError::SendRequestError)
        .map_fcm_err()?;

    if res.status().is_success() {
        debug!("FCM message sent successfully");
        Ok(())
    } else {
        let status = res.status().as_u16();
        let text = res
            .text()
            .await
            .map_err(NetworkError::ResponseError)
            .map_fcm_err()?;
        error!(
            "FCM message send successfully, but server returned an error. Status: {}, Response: {}",
            status, text
        );
        Err(NetworkError::ServerError(status, Some(text))).map_fcm_err()
    }
}

fn create_payload<T: Serialize>(
    device_token: &str,
    notification: Option<FcmNotification>,
    data_payload: Option<T>,
    apns_config: Option<ApnsConfig>,
) -> Result<serde_json::Value, FcmError> {
    // Start with base message
    let mut message = json!({
        "token": device_token
    });

    // Add notification if provided
    if let Some(notification) = notification {
        message["notification"] = json!({
            "title": notification.title,
            "body": notification.body
        });
    }

    // Add data payload if provided
    if let Some(data_payload) = data_payload {
        let data = serde_json::to_value(data_payload).map_err(FcmError::SerializationError)?;
        message["data"] = data;
    }

    // Add APNS config if provided
    if let Some(apns_config) = apns_config {
        message["apns"] =
            serde_json::to_value(apns_config).map_err(FcmError::SerializationError)?;
    }

    // Validate that we have at least one of: notification, data, or apns
    if message.get("notification").is_none()
        && message.get("data").is_none()
        && message.get("apns").is_none()
    {
        return Err(FcmError::FcmInvalidPayloadError);
    }

    Ok(json!({
        "message": message
    }))
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[tokio::test]
//     async fn test_create_payload_with_notification_and_data() {
//         let device_token = "test_device_token";
//         let notification = Some(FcmNotification {
//             title: "Test Title".to_string(),
//             body: "Test Body".to_string(),
//         });
//         let data_payload = Some(json!({
//             "key": "value"
//         }));

//         let payload = create_payload(device_token, notification, data_payload, None).unwrap();
//         assert_eq!(payload["message"]["token"], device_token);
//         assert_eq!(payload["message"]["notification"]["title"], "Test Title");
//         assert_eq!(payload["message"]["notification"]["body"], "Test Body");
//         assert_eq!(payload["message"]["data"]["key"], "value");
//     }

//     #[tokio::test]
//     async fn test_create_payload_with_only_notification() {
//         let device_token = "test_device_token";
//         let notification = Some(FcmNotification {
//             title: "Test Title".to_string(),
//             body: "Test Body".to_string(),
//         });
//         let data_payload: Option<serde_json::Value> = None;

//         let payload = create_payload(device_token, notification, data_payload, None).unwrap();
//         assert_eq!(payload["message"]["token"], device_token);
//         assert_eq!(payload["message"]["notification"]["title"], "Test Title");
//         assert_eq!(payload["message"]["notification"]["body"], "Test Body");
//         assert!(payload["message"]["data"].is_null());
//     }

//     #[tokio::test]
//     async fn test_create_payload_with_only_data() {
//         let device_token = "test_device_token";
//         let notification: Option<FcmNotification> = None;
//         let data_payload = Some(json!({
//             "key": "value"
//         }));

//         let payload = create_payload(device_token, notification, data_payload, None).unwrap();
//         assert_eq!(payload["message"]["token"], device_token);
//         assert!(payload["message"]["notification"].is_null());
//         assert_eq!(payload["message"]["data"]["key"], "value");
//     }

//     #[tokio::test]
//     async fn test_create_payload_with_silent_push() {
//         let device_token = "test_device_token";
//         let notification: Option<FcmNotification> = None;
//         let data_payload = Some(json!({
//             "key": "value"
//         }));
//         let apns_config = Some(ApnsConfig::silent_push());

//         let payload = create_payload(device_token, notification, data_payload, apns_config).unwrap();
//         assert_eq!(payload["message"]["token"], device_token);
//         assert_eq!(payload["message"]["apns"]["payload"]["aps"]["content-available"], 1);
//         assert_eq!(payload["message"]["data"]["key"], "value");
//     }

//     #[derive(serde::Serialize)]
//     struct TestData {
//         key1: String,
//         key2: String,
//     }

//     #[tokio::test]
//     async fn test_create_payload_with_only_struct_data() {
//         let device_token = "test_device_token";
//         let notification: Option<FcmNotification> = None;
//         let data_payload = TestData {
//             key1: "value1".to_string(),
//             key2: "value2".to_string(),
//         };

//         let payload = create_payload(device_token, notification, Some(data_payload), None).unwrap();
//         assert_eq!(payload["message"]["token"], device_token);
//         assert!(payload["message"]["notification"].is_null());
//         assert_eq!(payload["message"]["data"]["key1"], "value1");
//         assert_eq!(payload["message"]["data"]["key2"], "value2");
//     }

//     #[tokio::test]
//     async fn test_create_payload_with_no_notification_and_no_data() {
//         let device_token = "test_device_token";
//         let notification: Option<FcmNotification> = None;
//         let data_payload: Option<serde_json::Value> = None;

//         let payload = create_payload(device_token, notification, data_payload, None);
//         assert!(payload.is_err());
//     }

//     #[tokio::test]
//     async fn test_create_payload_with_only_apns() {
//         let device_token = "test_device_token";
//         let notification: Option<FcmNotification> = None;
//         let data_payload: Option<serde_json::Value> = None;
//         let apns_config = Some(ApnsConfig::silent_push());

//         let payload = create_payload(device_token, notification, data_payload, apns_config).unwrap();
//         assert_eq!(payload["message"]["token"], device_token);
//         assert_eq!(payload["message"]["apns"]["payload"]["aps"]["content-available"], 1);
//         assert!(payload["message"]["notification"].is_null());
//         assert!(payload["message"]["data"].is_null());
//     }
// }
