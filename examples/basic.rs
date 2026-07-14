use std::{env, error::Error, fs};

use phi::{Content, GenerationConfig, LlmProvider, Message, OpenAiChatProvider, ProviderRequest};

fn load_api_key() -> Result<String, Box<dyn Error>> {
    env::var("LLM_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .or_else(|| {
            fs::read_to_string(".dskey")
                .ok()
                .map(|key| key.trim().to_owned())
                .filter(|key| !key.is_empty())
        })
        .ok_or_else(|| "missing LLM_API_KEY".into())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenvy::dotenv().ok();

    let provider = OpenAiChatProvider::new(
        load_api_key()?,
        env::var("LLM_BASE_URL")?,
        env::var("LLM_MODEL")?,
    )?;
    let prompt = env::args()
        .nth(1)
        .unwrap_or_else(|| "用一句话介绍 Rust。".to_owned());
    let response = provider
        .generate(ProviderRequest {
            messages: vec![Message::user(prompt)],
            tools: Vec::new(),
            config: GenerationConfig {
                temperature: None,
                max_tokens: Some(4096),
                reasoning_effort: None,
            },
        })
        .await?;

    let answer = response
        .message
        .content
        .and_then(Content::into_text)
        .unwrap_or_else(|| "<no text response>".to_owned());
    println!("{answer}");
    if let Some(usage) = response.usage {
        eprintln!(
            "[usage] input={} output={} total={}",
            usage.input_tokens, usage.output_tokens, usage.total_tokens
        );
    }

    Ok(())
}
