// 用于 Runner / TestRunner 测试的程序。
//
// 行为：从标准输入逐行读取整数并求和，遇到空行或 EOF 时输出总和。
//   输入 "1\n2\n3\n\n"  -> 输出 "6"
//   输入 ""            -> 输出 "0"
use std::io::{self, BufRead};

fn main() {
    let stdin = io::stdin();
    let mut sum: i32 = 0;
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            break;
        }
        if let Ok(n) = line.trim().parse::<i32>() {
            sum += n;
        }
    }
    println!("{}", sum);
}
