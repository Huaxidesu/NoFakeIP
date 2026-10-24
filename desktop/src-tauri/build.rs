fn main() {
    // 图标变化要触发重新生成 Windows 资源。
    //
    // 背景：tauri-build 只声明监听 `tauri.conf.json` 和 `capabilities`，
    // 不监听图标文件。而 Windows 的 exe 图标是在构建脚本里编译
    // `resource.rc`（内部用绝对路径引用 icons/icon.ico）生成的。
    // 结果是：换了 icons/icon.ico 之后 cargo 认为构建脚本无需重跑，
    // exe 里仍然嵌着旧图标 —— 构建不报错，只有把图标抠出来比对才发现。
    //
    // 这里补上监听，让图标变更能自动生效。
    println!("cargo:rerun-if-changed=icons");
    println!("cargo:rerun-if-changed=tauri.conf.json");

    tauri_build::build()
}
