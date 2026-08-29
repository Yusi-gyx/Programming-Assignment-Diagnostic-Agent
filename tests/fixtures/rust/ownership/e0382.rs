// 触发 E0382 (borrow of moved value) 的样例程序。
// 对应知识点：Ownership / Move
fn main() {
    let s = String::from("hello");
    let t = s; // s 的所有权已移动到 t
    println!("{}", s); // 错误：使用了已移动的值
}
