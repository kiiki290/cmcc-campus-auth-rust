fn main() {
    // 前端资源在编译期嵌入二进制；显式声明依赖，否则只改 ui/ 时 cargo 会跳过重新编译。
    println!("cargo:rerun-if-changed=../ui");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    tauri_build::build()
}
