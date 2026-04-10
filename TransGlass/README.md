# TransGlass

轻量级 Windows 窗口透明度与置顶管理工具。

## 🚀 快速开始

1. **构建**: 
   ```bash
   cargo build --release
   ```
2. **运行**: 
   运行 `target/release/transglass.exe`。
3. **管理**:
   - 程序启动后将显示控制面板，点击关闭按钮将自动隐藏到系统托盘。
   - 左键点击托盘图标重新打开面板。
   - 右键点击托盘图标可进行一键还原或安全退出。

## ⌨️ 默认快捷键

| 功能 | 快捷键 |
| :--- | :--- |
| **增加透明度** | `Alt + Z` |
| **减少透明度** | `Alt + X` |
| **切换窗口置顶** | `Alt + T` |
| **还原当前窗口** | `Alt + R` |
| **还原所有窗口** | `Alt + Shift + R` |
| **重载热键配置** | `Alt + Shift + C` |
| **检查软件更新** | `Alt + U` |

## ⚙️ 自定义配置

你可以通过修改程序同级目录下的 `transglass_hotkeys.json` 来自定义热键。修改后点击面板上的“重载配置”或使用热键即可生效。

## 📦 打包安装

1. 确保已安装 [Inno Setup](https://jrsoftware.org/isinfo.php)。
2. 运行 `cargo build --release`。
3. 使用 Inno Setup 编译 `TransGlass.iss` 即可生成标准安装程序。
