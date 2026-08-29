// 触发 E0499 (cannot borrow as mutable more than once) 的样例程序。
// 对应知识点：Borrowing / Mutable Borrow
fn main() {
    let mut s = String::from("hello");
    let r1 = &mut s;
    let r2 = &mut s; // 错误：不能同时存在两个可变借用
    println!("{} {}", r1, r2);
}
