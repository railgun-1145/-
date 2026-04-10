# TransGlass 技术溯源与架构大纲

本文档旨在为 TransGlass 项目提供完整的技术溯源，确保在不同环境下编译时能够准确还原所有功能逻辑。

## 1. 项目核心元数据
- **项目名称**: TransGlass
- **核心定位**: 轻量级 Windows 窗口透明度与置顶管理工具
- **源码路径**: `E:\git_ku\TransGlass` (推荐避免路径空格)
- **开发语言**: Rust (Edition 2021)
- **目标平台**: Windows 10/11 (x64)
- **开源地址**: https://github.com/railgun-1145/TransGlass

## 2. 依赖溯源 (Dependency Graph)
项目主要依赖以下 Rust Crate，确保在编译时版本一致：
- `windows`: 直接交互 Win32 API (HWND, Styles, Events)。
- `eframe/egui`: 构建硬件加速的现代化 GUI 面板。
- `dashmap`: 线程安全的窗口状态并发注册表。
- `tray-icon`: 系统托盘与上下文菜单支持。
- `self_update`: 实现基于 GitHub Releases 的自动更新。
- `serde/serde_json`: 配置文件 (`transglass_hotkeys.json`) 的序列化。

## 3. 文件功能映射 (Source Mapping)
- `src/main.rs`: **主程序入口**。包含热键监听循环、GUI 生命周期管理、托盘事件处理及自更新逻辑。
- `Cargo.toml`: **构建配置文件**。定义了项目版本、编译优化参数及上述所有依赖项。
- `transglass.manifest`: **Windows 清单文件**。强制程序请求管理员权限 (UAC) 并声明 DPI 感知 (防止 4K 屏模糊)。
- `build.rs`: **编译脚本**。在构建过程中自动将清单文件嵌入生成的 `.exe` 中。
- `TransGlass.iss`: **安装包脚本**。使用 Inno Setup 生成标准安装/卸载程序。

## 4. 核心逻辑溯源
### 4.1 窗口注册表 (Window Registry)
所有对系统窗口的修改均通过 `GLOBAL_REGISTRY` (基于 `DashMap`) 进行追踪。
- **记录内容**: 原始 `WS_EX_STYLE`、原始置顶状态、当前透明度、用户偏好。
- **还原保证**: 程序退出或窗口销毁时，读取注册表数据并调用 `SetWindowLongW` 恢复原始样式。

### 4.2 热键处理循环
程序在独立线程中运行 Win32 消息循环 (`GetMessageW`)，监听由 `RegisterHotKey` 注册的系统级全局热键。

### 4.3 GUI 渲染
`egui` 每 100ms 轮询一次注册表状态，确保界面上的滑动条与热键调节同步。

## 5. 构建与打包流程
1. **环境准备**: 安装 Rust 工具链 (MSVC) 及 Inno Setup。
2. **编译源码**: 
   ```bash
   cargo build --release
   ```
3. **分发文件**: 
   所有关键分发文件已汇总至 `E:\daima\002\TransGlass_Distribution` 文件夹。
4. **生成安装包**: 
   运行 Inno Setup 并加载 `E:\daima\002\TransGlass_Distribution\TransGlass.iss`，编译生成安装程序。

## 6. 安全与一致性保证
- **100% 还原**: 程序设计原则是“不留痕迹”，所有样式修改在关闭时必须清除。
- **路径无关性**: `TransGlass.iss` 使用相对路径变量，支持在任何开发目录下直接打包。
