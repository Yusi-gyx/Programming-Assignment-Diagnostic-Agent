//! LLM 真实调用示例
//!
//! 从环境变量 DEEPSEEK_API_KEY 读取 key，调用 DeepSeek API，
//! 用项目自身的 LlmClient 完成一次「Rust 错误诊断」对话。
//!
//! 运行：
//! ```bash
//! source /tmp/opencode/pada_deepseek_key.sh && cargo run --example llm_demo
//! ```

use PADA::agent::llm::{ChatMessage, LlmClient};
use PADA::config::model::ModelConfig;

fn main() {
    let api_key = std::env::var("DEEPSEEK_API_KEY").unwrap_or_else(|_| {
        eprintln!("错误：未设置环境变量 DEEPSEEK_API_KEY");
        eprintln!("请先运行: source /tmp/opencode/pada_deepseek_key.sh");
        std::process::exit(1);
    });

    // 构造 DeepSeek 模型配置
    let config = ModelConfig::cloud(
        "https://api.deepseek.com/v1/chat/completions",
        api_key,
        "deepseek-chat",
        64000,
        1.0,
        2.0,
    );

    let client = LlmClient::new(config);

    // 构造一段「Rust 错误诊断」对话
    let messages = vec![
        ChatMessage::system(
            "你是一位 Rust 编程导师。用户会给出错误代码和编译器报错，\
             你要用简洁的中文指出问题所在与相关知识点，不要直接给出完整答案。",
        ),
        ChatMessage::user(
            "我的 Rust 代码报错了：\n\n\
             ```rust\n\
             fn main() {\n\
                 let s = String::from(\"hello\");\n\
                 let t = s;\n\
                 println!(\"{}\", s);\n\
             }\n```\n\n\
             编译器报错：error[E0382]: borrow of moved value: `s`\n\n\
             请帮我分析。",
        ),
    ];

    // 打印请求体（让你看到实际发送的 JSON 结构）
    println!("========== 请求体 ==========");
    let body = client.build_request_body(&messages);
    println!("{}", serde_json::to_string_pretty(&body).unwrap());
    println!();

    // 发送请求
    println!("========== 调用中... ==========");
    let start = std::time::Instant::now();
    match client.chat(&messages) {
        Ok(resp) => {
            let elapsed = start.elapsed();
            println!("========== 响应内容 ==========");
            println!("{}", resp.content);
            println!();
            println!("========== 用量与耗时 ==========");
            println!("模型        : {}", resp.model);
            println!("输入 tokens : {}", resp.input_tokens);
            println!("输出 tokens : {}", resp.output_tokens);
            // 成本换算（每百万 token 的价格）
            let cost_in = resp.input_tokens as f64 * 1.0 / 1_000_000.0;
            let cost_out = resp.output_tokens as f64 * 2.0 / 1_000_000.0;
            println!("成本(元)    : {:.6} (输入 {:.6} + 输出 {:.6})",
                cost_in + cost_out, cost_in, cost_out);
            println!("耗时        : {:.2?}", elapsed);
        }
        Err(e) => {
            eprintln!("调用失败: {}", e);
            std::process::exit(1);
        }
    }
}
