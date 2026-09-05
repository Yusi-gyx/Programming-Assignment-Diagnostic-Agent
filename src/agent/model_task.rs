//! 可取消的交互式模型任务。

use crate::agent::llm::{ChatMessage, ChatModel, LlmResponse, ModelTaskKind};
use crate::error::Result;
use crate::history::{Session, StepBuilder, ToolCall};
use std::io;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct FailedModelCall {
    pub total_ms: u64,
    pub cancelled: bool,
    pub error: String,
}

/// 失败和取消也进入轨迹；没有 API usage 时不猜测 Token，不保存未完成正文。
pub fn run_recorded_model_task(
    model: Arc<dyn ChatModel>,
    messages: &[ChatMessage],
    interactive: bool,
    task: ModelTaskKind,
    session: &mut Session,
    on_chunk: impl FnMut(&str) + Send,
) -> ModelTaskOutcome {
    let started = std::time::Instant::now();
    let outcome = run_model_task_streaming_for_kind(model, messages, interactive, task, on_chunk);
    let error = match &outcome {
        ModelTaskOutcome::Completed(Ok(_)) => return outcome,
        ModelTaskOutcome::Completed(Err(error)) => error.to_string(),
        ModelTaskOutcome::Cancelled => "用户取消模型调用；服务端最终用量未知".into(),
    };
    let failure = FailedModelCall {
        total_ms: started.elapsed().as_millis() as u64,
        cancelled: matches!(outcome, ModelTaskOutcome::Cancelled),
        error,
    };
    eprintln!(
        "模型调用{}，已等待 {:.2} 秒；未取得完整 API 用量。",
        if failure.cancelled {
            "已取消"
        } else {
            "失败"
        },
        failure.total_ms as f64 / 1000.0
    );
    session.add_step(
        StepBuilder::new(session.step_count())
            .tool_call(ToolCall::new(
                "llm_failed_call",
                serde_json::json!({"task": format!("{task:?}"), "messages": messages}).to_string(),
                serde_json::json!(failure).to_string(),
            ))
            .build(),
    );
    outcome
}

pub enum ModelTaskOutcome {
    Completed(Result<LlmResponse>),
    Cancelled,
}

/// 执行模型调用。交互终端中可输入 `q` / `cancel` 并回车停止流式生成。
pub fn run_model_task(
    model: Arc<dyn ChatModel>,
    messages: &[ChatMessage],
    interactive: bool,
) -> ModelTaskOutcome {
    run_model_task_streaming(model, messages, interactive, |_| {})
}

pub fn run_model_task_streaming(
    model: Arc<dyn ChatModel>,
    messages: &[ChatMessage],
    interactive: bool,
    on_chunk: impl FnMut(&str) + Send,
) -> ModelTaskOutcome {
    run_model_task_streaming_for_kind(
        model,
        messages,
        interactive,
        ModelTaskKind::General,
        on_chunk,
    )
}

pub fn run_model_task_streaming_for_kind(
    model: Arc<dyn ChatModel>,
    messages: &[ChatMessage],
    interactive: bool,
    task: ModelTaskKind,
    mut on_chunk: impl FnMut(&str) + Send,
) -> ModelTaskOutcome {
    if !interactive {
        let cancelled = AtomicBool::new(false);
        return ModelTaskOutcome::Completed(model.chat_for_task(
            messages,
            task,
            &cancelled,
            &mut on_chunk,
        ));
    }

    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let messages = messages.to_vec();
    enum Event {
        Chunk(String),
        Finished(Result<LlmResponse>),
    }
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let chunk_sender = sender.clone();
        let result = model.chat_for_task(&messages, task, &worker_cancelled, &mut |chunk| {
            let _ = chunk_sender.send(Event::Chunk(chunk.to_owned()));
        });
        let _ = sender.send(Event::Finished(result));
    });

    let mut input_available = true;
    loop {
        match receiver.try_recv() {
            Ok(Event::Chunk(chunk)) => {
                on_chunk(&chunk);
                continue;
            }
            Ok(Event::Finished(result)) => return ModelTaskOutcome::Completed(result),
            Err(mpsc::TryRecvError::Disconnected) => {
                return ModelTaskOutcome::Completed(Err(crate::error::PadaError::Llm(
                    "模型任务线程意外结束".into(),
                )));
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }

        if input_available && stdin_ready(100).unwrap_or(false) {
            let mut input = String::new();
            match io::stdin().read_line(&mut input) {
                Ok(0) => input_available = false,
                Ok(_) if is_cancel_command(&input) || matches!(input.trim(), "exit" | "quit") => {
                    cancelled.store(true, Ordering::Release);
                    eprintln!("⏹ 已请求停止模型生成，正在返回导师模式…");
                    return ModelTaskOutcome::Cancelled;
                }
                Ok(_) => eprintln!("模型仍在生成；输入 q 或 cancel 并回车可停止。"),
                Err(_) => input_available = false,
            }
        } else if !input_available {
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(Event::Chunk(chunk)) => {
                    on_chunk(&chunk);
                }
                Ok(Event::Finished(result)) => return ModelTaskOutcome::Completed(result),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return ModelTaskOutcome::Completed(Err(crate::error::PadaError::Llm(
                        "模型任务线程意外结束".into(),
                    )));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

pub fn is_cancel_command(input: &str) -> bool {
    matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "q" | "cancel" | "取消"
    )
}

#[cfg(unix)]
pub(crate) fn stdin_ready(timeout_ms: i32) -> io::Result<bool> {
    let mut descriptor = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: descriptor 指向一个有效的 pollfd，数量为 1，调用期间保持存活。
    let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(result > 0 && descriptor.revents & libc::POLLIN != 0)
}

#[cfg(not(unix))]
pub(crate) fn stdin_ready(_timeout_ms: i32) -> io::Result<bool> {
    Ok(false)
}
