//! Live end-to-end probe against a real OpenAI-compatible endpoint, exercising
//! the `OpenAIChatModel` generate / stream / tool_use code paths that the
//! in-crate `#[cfg(test)]` suite can only cover with fixtures (and that
//! `cargo test --lib` can't run at all on this box — the app_lib test exe fails
//! to load with STATUS_ENTRYPOINT_NOT_FOUND). An example binary is a plain exe,
//! not the test harness, so it sidesteps that loader issue and runs the real
//! HTTP + SSE + tool-call-merge code against a live server.
//!
//! The endpoint is fully parameterized so ANY OpenAI-compatible provider works
//! — OpenAI, DeepSeek, OpenRouter, minimax, etc. No key is ever persisted;
//! pass it via env vars that live only in the shell that runs this:
//!
//! ```sh
//! # minimax (the default endpoint)
//! OPENAI_API_KEY=sk-... cargo run --example openai_live --release
//! # deepseek (override base + model)
//! OPENAI_API_KEY=sk-... OPENAI_BASE_URL=https://api.deepseek.com \
//!   OPENAI_MODEL=deepseek-v4-flash cargo run --example openai_live --release
//! ```
//!
//! Exit 0 = all three paths produced the expected shapes:
//!   1. generate (non-stream)   — assistant content non-empty
//!   2. stream                  — >=1 delta, accumulated content non-empty
//!   3. generate + tools        — model emits a get_weather tool_call whose
//!                                arguments are valid JSON with a city field
//! Non-zero = a path failed; the printed line names which.

use std::error::Error;

use app_lib::kernel_impl::openai_chat_model::OpenAIChatModel;
use futures::StreamExt;
use kernel_core::schema::Message;
use kernel_core::{ChatModel, ModelOptions, ToolInfo};

const DEFAULT_BASE: &str = "https://api.minimaxi.com/v1";
const DEFAULT_MODEL: &str = "minimax-m3";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("MINIMAX_API_KEY"))
        .map_err(|_| "set OPENAI_API_KEY (or MINIMAX_API_KEY); env-only, never persisted")?;
    let base = std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE.to_string());
    let model_name = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
    println!("endpoint = {base}\nmodel    = {model_name}");
    let model = OpenAIChatModel::new(&base, key, &model_name);

    // Reasoning models differ in how they surface thought: minimax-m3 inlines
    // <think>…</think> inside `content`; DeepSeek-V4 puts it in a separate
    // `reasoning_content` field (which this OpenAI layer ignores — it stays out
    // of `content`). Either way max_tokens must budget for reasoning + answer.
    let text_opts = ModelOptions {
        max_tokens: Some(256),
        temperature: Some(0.0),
        ..Default::default()
    };
    let ping = vec![
        Message::system("Reply with exactly: pong. Nothing else."),
        Message::user("ping"),
    ];

    // --- 1. generate: non-streaming text ------------------------------------
    println!("[1/3] generate (non-stream)…");
    let resp = model.generate(&ping, &text_opts).await?;
    println!("    content    = {:?}", resp.content);
    println!("    tool_calls = {}", resp.tool_calls.len());
    assert!(
        !resp.content.trim().is_empty(),
        "generate returned empty content"
    );

    // --- 2. stream: streaming text deltas -----------------------------------
    println!("[2/3] stream (text deltas)…");
    let mut stream = model.stream(&ping, &text_opts)?;
    let mut acc = String::new();
    let mut chunks = 0u32;
    while let Some(delta) = stream.next().await {
        let delta = delta?;
        chunks += 1;
        if !delta.content.is_empty() {
            acc.push_str(&delta.content);
        }
        if !delta.tool_calls.is_empty() {
            println!("    [stream] mid-stream tool_call: {:?}", delta.tool_calls);
        }
    }
    println!("    chunks = {}", chunks);
    println!("    acc    = {:?}", acc);
    assert!(chunks >= 1, "stream yielded no deltas");
    assert!(!acc.trim().is_empty(), "stream accumulated empty content");

    // --- 3. generate + tools: tool_call round-trip --------------------------
    println!("[3/3] generate with tools (tool_call round-trip)…");
    let weather = ToolInfo {
        name: "get_weather".into(),
        description: "Get the current weather for a given city.".into(),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "city": { "type": "string", "description": "City name" },
                "unit": { "type": "string", "enum": ["celsius", "fahrenheit"] }
            },
            "required": ["city"]
        }),
    };
    let tooled = model.with_tools(&[weather])?;
    let tool_msgs = vec![Message::user(
        "What is the weather in Beijing right now? You MUST call the get_weather tool.",
    )];
    // tool_calls arrive alongside (or after) reasoning; give the call room.
    let tool_opts = ModelOptions {
        max_tokens: Some(512),
        temperature: Some(0.0),
        ..Default::default()
    };
    let tool_resp = tooled.generate(&tool_msgs, &tool_opts).await?;
    println!("    content    = {:?}", tool_resp.content);
    println!("    tool_calls = {:?}", tool_resp.tool_calls);
    assert!(
        !tool_resp.tool_calls.is_empty(),
        "model did not call get_weather — content was {:?}",
        tool_resp.content
    );
    let tc = &tool_resp.tool_calls[0];
    assert_eq!(
        tc.function.name, "get_weather",
        "tool name mismatch (got {:?})",
        tc.function.name
    );
    // arguments is a JSON-encoded STRING on the wire (fragments concatenated
    // during streaming); verify it parses and carries the expected city.
    let args: serde_json::Value = serde_json::from_str(&tc.function.arguments).map_err(|e| {
        format!(
            "arguments not valid JSON: {e}\nraw: {:?}",
            tc.function.arguments
        )
    })?;
    println!("    parsed args = {}", args);
    let city = args.get("city").and_then(|v| v.as_str()).unwrap_or("");
    let looks_like_beijing = city.to_lowercase().contains("beijing") || city.contains('北');
    assert!(looks_like_beijing, "city arg {:?} is not Beijing", city);

    println!(
        "\nALL OK — OpenAI protocol generate + stream + tool_use verified end-to-end against {model_name}."
    );
    Ok(())
}
