//! Poll subprocesses while draining pipes, so input and output cannot block cancellation.
use crate::error::{PadaError, Result};
use std::io::{Read, Write};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

pub fn terminal_cancelled() -> bool {
    if !crate::agent::model_task::stdin_ready(0).unwrap_or(false) {
        return false;
    }
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "q" | "quit" | "exit" | "cancel" | "取消"
    )
}

struct ManagedChild(Child);
impl Drop for ManagedChild {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            // Each command starts in its own process group; include descendants.
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn run_command(
    command: &mut Command,
    input: &[u8],
    timeout: Option<Duration>,
    mut cancelled: impl FnMut() -> bool,
) -> Result<Output> {
    if cancelled() {
        return Err(PadaError::Cancelled);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = ManagedChild(command.spawn()?);
    let mut stdin = child.0.stdin.take();
    let mut stdout = child.0.stdout.take();
    let mut stderr = child.0.stderr.take();
    let input = input.to_vec();
    let writer = std::thread::spawn(move || {
        if let Some(stdin) = &mut stdin {
            let _ = stdin.write_all(&input);
        }
    });
    let out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(stdout) = &mut stdout {
            stdout.read_to_end(&mut bytes)?;
        }
        Ok::<_, std::io::Error>(bytes)
    });
    let err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(stderr) = &mut stderr {
            stderr.read_to_end(&mut bytes)?;
        }
        Ok::<_, std::io::Error>(bytes)
    });
    let started = Instant::now();
    let result = loop {
        if cancelled() {
            break Err(PadaError::Cancelled);
        }
        if timeout.is_some_and(|limit| started.elapsed() >= limit) {
            break Err(PadaError::Run("子进程执行超时".into()));
        }
        match child.0.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => break Err(error.into()),
        }
    };
    drop(child);
    let _ = writer.join();
    let stdout = out
        .join()
        .map_err(|_| PadaError::Run("stdout 读取线程失败".into()))?;
    let stderr = err
        .join()
        .map_err(|_| PadaError::Run("stderr 读取线程失败".into()))?;
    Ok(Output {
        status: result?,
        stdout: stdout?,
        stderr: stderr?,
    })
}
