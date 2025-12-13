use reqwest::Client;
use serde_json::json;
use std::env;

pub async fn ask_ai(context: String) -> Result<String, String> {
    // Check if API Key is set
    let api_key = match env::var("OPENAI_API_KEY") {
        Ok(k) => k,
        Err(_) => return Err("OPENAI_API_KEY environment variable not set. Please set it to use AI features.".to_string()),
    };
    
    let endpoint = env::var("KORADAR_AI_ENDPOINT").unwrap_or_else(|_| "https://api.openai.com/v1/chat/completions".to_string());
    let model = env::var("KORADAR_AI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());

    let client = Client::new();
    let prompt = format!(
        "You are a binary analysis expert. Explain what is happening in the following execution context of a program trace.\n\nContext:\n{}",
        context
    );

    let body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": "You are a helpful assistant for binary analysis. Be concise and technical."},
            {"role": "user", "content": prompt}
        ]
    });

    let res = client.post(&endpoint)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(format!("API Error {}: {}", status, text));
    }

    let json: serde_json::Value = res.json().await.map_err(|e| format!("Parse failed: {}", e))?;
    
    // Extract content
    json["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Invalid response format".to_string())
}


