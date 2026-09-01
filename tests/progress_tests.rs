//! 进度渲染与任务打断测试（R4 / 第 13 步）
//!
//! 全部离线测试，验证取消令牌与进度接口逻辑。
//!
//! ```bash
//! cargo test --test progress_tests
//! ```

use pada::agent::progress::{CancelToken, DiagnosticStage, ProgressReporter, SilentProgress};
use std::sync::Arc;
use std::thread;

// ============================================================
// CancelToken 测试
// ============================================================

#[test]
fn test_cancel_token_initial_not_cancelled() {
    let token = CancelToken::new();
    assert!(!token.is_cancelled(), "新建的令牌不应处于取消状态");
}

#[test]
fn test_cancel_token_cancel() {
    let token = CancelToken::new();
    token.cancel();
    assert!(token.is_cancelled(), "cancel 后应处于取消状态");
}

#[test]
fn test_cancel_token_clone_shares_state() {
    // 克隆的令牌应共享同一状态
    let token = CancelToken::new();
    let clone = token.clone();

    // 在原令牌上取消
    token.cancel();

    // 克隆应也观察到取消
    assert!(clone.is_cancelled(), "克隆令牌应共享取消状态");
    assert!(token.is_cancelled());
}

#[test]
fn test_cancel_token_thread_safety() {
    // 多线程下的取消可见性
    let token = CancelToken::new();
    let worker = token.clone();

    let handle = thread::spawn(move || {
        let mut count = 0;
        while !worker.is_cancelled() {
            count += 1;
            if count > 1_000_000 {
                break;
            }
        }
        count
    });

    // 主线程取消
    thread::sleep(std::time::Duration::from_millis(1));
    token.cancel();

    let count = handle.join().unwrap();
    assert!(count <= 1_000_000, "取消后工作线程应停止");
}

#[test]
fn test_cancel_token_default() {
    let token = CancelToken::default();
    assert!(!token.is_cancelled());
}

#[test]
fn test_cancel_token_multiple_cancel_calls() {
    // 多次 cancel 不应出错
    let token = CancelToken::new();
    token.cancel();
    token.cancel();
    token.cancel();
    assert!(token.is_cancelled());
}

#[test]
fn test_cancel_token_arc_sharing() {
    // 通过 Arc 共享也应是同一状态
    let token = Arc::new(CancelToken::new());
    let token2 = Arc::clone(&token);

    token2.cancel();
    assert!(token.is_cancelled());
}

// ============================================================
// SilentProgress 测试
// ============================================================

#[test]
fn test_silent_progress_does_not_panic() {
    let progress = SilentProgress;
    progress.start(10, "测试任务");
    progress.tick(5, "第 5 步");
    progress.finish("完成");
    progress.cancelled("用户取消");
}

#[test]
fn test_silent_progress_clone() {
    let p1 = SilentProgress;
    let p2 = p1.clone();
    p1.start(5, "task1");
    p2.tick(3, "task2");
    // 不 panic 即可
}

// ============================================================
// ProgressReporter trait 对象测试
// ============================================================

#[test]
fn test_progress_reporter_as_dyn() {
    // trait 对象应可用
    let reporter: Box<dyn ProgressReporter> = Box::new(SilentProgress);
    reporter.start(3, "任务");
    reporter.tick(1, "步骤 1");
    reporter.tick(2, "步骤 2");
    reporter.finish("完成");
}

#[test]
fn test_progress_reporter_with_cancel() {
    // 模拟：使用取消令牌 + 静默进度的工作流
    let cancel = CancelToken::new();
    let progress = SilentProgress;
    let total = 10;

    progress.start(total, "批量任务");

    let mut completed = 0;
    for i in 0..total {
        // 关键检查点：检查取消
        if cancel.is_cancelled() {
            progress.cancelled("用户请求");
            break;
        }
        // 执行工作...
        completed = i + 1;
        progress.tick(completed, &format!("第 {}/{}", completed, total));
    }

    // 未取消时应完成全部
    assert_eq!(completed, total);

    // 现在取消并重新运行
    cancel.cancel();
    assert!(cancel.is_cancelled());

    let mut completed2 = 0;
    for i in 0..total {
        if cancel.is_cancelled() {
            progress.cancelled("用户请求");
            break;
        }
        completed2 = i + 1;
    }
    // 取消后应立即停止
    assert_eq!(completed2, 0, "取消后不应执行任何工作");
}

// ============================================================
// DiagnosticStage 测试
// ============================================================

#[test]
fn test_diagnostic_stage_as_str() {
    assert_eq!(DiagnosticStage::ReadingInput.as_str(), "读取输入");
    assert_eq!(DiagnosticStage::Compiling.as_str(), "编译代码");
    assert_eq!(DiagnosticStage::ParsingErrors.as_str(), "解析错误");
    assert_eq!(DiagnosticStage::RunningTests.as_str(), "运行测试");
    assert_eq!(DiagnosticStage::LlmCalling.as_str(), "调用 LLM");
    assert_eq!(DiagnosticStage::GeneratingReport.as_str(), "生成报告");
}

#[test]
fn test_diagnostic_stage_eq() {
    assert_eq!(DiagnosticStage::Compiling, DiagnosticStage::Compiling);
    assert_ne!(DiagnosticStage::Compiling, DiagnosticStage::RunningTests);
}

// ============================================================
// 协作式取消集成测试（模拟批量测试运行）
// ============================================================

#[test]
fn test_cooperative_cancellation_in_batch() {
    use pada::tools::runner::{TestCase, TestRunner};

    // 编译一个测试程序
    use pada::tools::compiler::CompilerTool;
    use std::path::PathBuf;
    fn fixture(relative: &str) -> PathBuf {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("tests/fixtures/rust");
        p.push(relative);
        p
    }

    let compiler = CompilerTool::new();
    let program = std::env::temp_dir().join("pada_cancel_test");
    compiler
        .compile_file(&fixture("runner/io_program.rs"), Some(&program))
        .unwrap();

    // 准备 5 组测试用例
    let tests: Vec<TestCase> = (0..5)
        .map(|i| {
            TestCase::new(
                format!("case_{}", i),
                format!("{}\n\n", i),
                format!("{}", i),
            )
        })
        .collect();

    // 在第 3 组后取消
    let cancel = CancelToken::new();
    let progress = SilentProgress;
    progress.start(tests.len(), "运行测试");

    let test_runner = TestRunner::new();
    let mut completed = 0;
    for (i, tc) in tests.iter().enumerate() {
        if cancel.is_cancelled() {
            progress.cancelled("用户取消");
            break;
        }
        let _ = test_runner.run_tests(&program, std::slice::from_ref(tc));
        completed = i + 1;
        progress.tick(completed, &format!("第 {}/{} 组", completed, tests.len()));

        // 在第 3 组后触发取消
        if i == 2 {
            cancel.cancel();
        }
    }

    // 应执行了 3 组（第 0、1、2），第 3 组因取消而停止
    assert_eq!(completed, 3, "取消后应只完成 3 组");
}
