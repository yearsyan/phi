use std::{env, error::Error, fs, path::Path};

use phi::{Agent, ImageDetail, ImageUrl, OpenAiChatProvider};

fn load_image(source: &str) -> Result<ImageUrl, Box<dyn Error>> {
    if source.starts_with("http://")
        || source.starts_with("https://")
        || source.starts_with("data:")
    {
        return Ok(ImageUrl::new(source).with_detail(ImageDetail::Auto));
    }

    let mime_type = match Path::new(source)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => return Err("supported image extensions: png, jpg, jpeg, gif, webp".into()),
    };

    Ok(ImageUrl::from_bytes(mime_type, &fs::read(source)?).with_detail(ImageDetail::Auto))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenvy::dotenv().ok();

    let api_key = env::var("VISION_API_KEY")?;
    let base_url = env::var("VISION_BASE_URL")?;
    let model = env::var("VISION_MODEL")?;
    let image_source = env::args()
        .nth(1)
        .ok_or("usage: cargo run --example vision -- <image-path-or-url> [prompt]")?;
    let prompt = env::args()
        .nth(2)
        .unwrap_or_else(|| "Describe this image concisely.".to_owned());

    let max_context_tokens = env::var("VISION_MAX_CONTEXT_TOKENS")?.parse()?;
    let provider = OpenAiChatProvider::new(api_key, base_url, model)?;
    let mut agent = Agent::builder(provider)
        .max_context_tokens(max_context_tokens)
        .build();
    let result = agent
        .prompt_with_images(prompt, vec![load_image(&image_source)?])
        .await?;

    println!("{}", result.text().unwrap_or("<no text response>"));
    Ok(())
}
