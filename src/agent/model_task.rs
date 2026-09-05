//! 可取消的交互式模型任务。

use crate::agent::llm::{ChatMessage, ChatModel, LlmResponse};
use crate::error::Result;
use std::io;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

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
    mut on_chunk: impl FnMut(&str) + Send,
) -> ModelTaskOutcome {
    if !interactive {
        let cancelled = AtomicBool::new(false);
        return ModelTaskOutcome::Completed(model.chat_cancellable_streaming(
            messages,
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
        let result = model.chat_cancellable_streaming(&messages, &worker_cancelled, &mut |chunk| {
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
