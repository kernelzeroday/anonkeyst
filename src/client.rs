use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};

const BASE_URL: &str = "https://anonkey.st/v1";

fn api_error(status: reqwest::StatusCode, body: &str) -> String {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(msg) = json.get("error").and_then(|e| {
            e.get("message").and_then(|m| m.as_str()).or_else(|| e.as_str())
        }) {
            return msg.to_string();
        }
        if let Some(msg) = json.get("message").and_then(|m| m.as_str()) {
            return msg.to_string();
        }
    }
    format!("request failed (HTTP {}): {}", status.as_u16(), body)
}

pub struct Client {
    http: reqwest::Client,
    api_key: Option<String>,
}

#[derive(Deserialize)]
struct AccountResponse {
    api_key: String,
}


#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

#[derive(Deserialize)]
pub struct DepositPolicy {
    pub asset: String,
    pub network: String,
}

#[derive(Deserialize)]
struct DepositPoliciesResponse {
    data: Vec<DepositPolicy>,
}

#[derive(Serialize)]
struct DepositDestinationRequest {
    asset: String,
    network: String,
}

#[derive(Deserialize)]
struct DepositDestinationResponse {
    address: String,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: String,
}

impl Client {
    pub fn new(api_key: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key: Some(api_key.to_string()),
        }
    }

    pub fn unauthenticated() -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key: None,
        }
    }

    fn auth_header(&self) -> Result<String, Box<dyn std::error::Error>> {
        let key = self.api_key.as_ref().ok_or("no API key configured")?;
        Ok(format!("Bearer {}", key))
    }

    pub async fn create_account(&self) -> Result<String, Box<dyn std::error::Error>> {
        let resp = self
            .http
            .post(format!("{}/accounts", BASE_URL))
            .header(CONTENT_TYPE, "application/json")
            .body("{}")
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(api_error(status, &body).into());
        }

        let data: AccountResponse = resp.json().await?;
        Ok(data.api_key)
    }

    pub async fn get_balance(&self) -> Result<String, Box<dyn std::error::Error>> {
        let resp = self
            .http
            .get(format!("{}/balance", BASE_URL))
            .header(AUTHORIZATION, self.auth_header()?)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(api_error(status, &body).into());
        }

        let text = resp.text().await?;
        let json: serde_json::Value = serde_json::from_str(&text)
            .map_err(|_| format!("unexpected response: {}", text))?;

        let balance = if let Some(b) = json.get("balance") {
            match b {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                other => other.to_string(),
            }
        } else {
            json.to_string()
        };

        Ok(balance)
    }

    pub async fn list_models(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let resp = self
            .http
            .get(format!("{}/models", BASE_URL))
            .header(AUTHORIZATION, self.auth_header()?)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(api_error(status, &body).into());
        }

        let data: ModelsResponse = resp.json().await?;
        Ok(data.data.into_iter().map(|m| m.id).collect())
    }

    pub async fn get_deposit_policies(
        &self,
    ) -> Result<Vec<DepositPolicy>, Box<dyn std::error::Error>> {
        let resp = self
            .http
            .get(format!("{}/billing/deposit-policies", BASE_URL))
            .header(AUTHORIZATION, self.auth_header()?)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(api_error(status, &body).into());
        }

        let data: DepositPoliciesResponse = resp.json().await?;
        Ok(data.data)
    }

    pub async fn create_deposit_destination(
        &self,
        asset: &str,
        network: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let body = DepositDestinationRequest {
            asset: asset.to_string(),
            network: network.to_string(),
        };

        let resp = self
            .http
            .post(format!("{}/billing/deposit-destinations", BASE_URL))
            .header(AUTHORIZATION, self.auth_header()?)
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(api_error(status, &body).into());
        }

        let data: DepositDestinationResponse = resp.json().await?;
        Ok(data.address)
    }

    pub async fn chat(
        &self,
        model: &str,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let body = ChatRequest {
            model: model.to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: message.to_string(),
            }],
        };

        let resp = self
            .http
            .post(format!("{}/chat/completions", BASE_URL))
            .header(AUTHORIZATION, self.auth_header()?)
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(api_error(status, &body).into());
        }

        let data: ChatResponse = resp.json().await?;
        let content = data
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_else(|| "(no response)".to_string());
        Ok(content)
    }
}
