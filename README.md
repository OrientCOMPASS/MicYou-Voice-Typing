# ⌨️ Voice-Typing — MicYou 语音输入法插件

> 为 [MicYou 桌面端（Windows）](https://github.com/LanRhyme/MicYou) 打造的语音输入插件，让你「说话即打字」。

按下快捷键开口说话，识别出的文本自动输入到当前焦点输入框——写文档、回消息、填表单，双手离开键盘。


## 📖 工作原理

**Voice-Typing** 基于 [WhatdidIsay (WDIS)](https://github.com/OrientCOMPASS/WhatdidIsay) 的全局语音转录广播：

- WDIS 在后台持续进行离线语音识别，并把每句话（含起止时间戳）广播到插件总线；
- Voice-Typing 维护一个**抗延迟时间窗口状态机**：只有落在「快捷键激活窗口」内的句子才会被输入，激活前后误识别的环境语音会被过滤；
- 通过 **UI Automation 光标追踪**在光标旁绘制红色麦克风标记，并用 `SendInput`（Unicode 注入）把文本输入到当前焦点输入框。

## 📦 安装

1. **先安装依赖**：安装并启用 [WhatdidIsay](https://github.com/OrientCOMPASS/WhatdidIsay) 插件（本插件的 manifest 已声明依赖，缺失时宿主会阻止启用）
2. 下载 Release 中的 `opss.voice-typing.zip`（开发版见每次 push 构建的 Actions artifact `voice-typing-development`）
3. 在 MicYou「设置 → 插件」点「导入插件」选择该 zip；或解压到插件目录的 `opss.voice-typing/` 子目录（`%APPDATA%\micyou\plugins\`）
4. 回到插件页点「刷新」，启用 **Voice-Typing**
5. 应用内「检查更新」走 `updateUrl`（release 资产 `plugin.json`），一键更新按 `downloadUrl` 下载 latest release 的 zip

## 🚀 使用

1. 把光标点进目标输入框（任意应用）
2. 按下快捷键（默认 **左 Ctrl + 左 Shift**），光标旁出现红色麦克风标记：
   - **按键切换模式（默认）**：按一次激活，再按一次结束，适合连续长句输入
   - **长按说话模式**：按住快捷键期间输入，松开即结束，适合碎片化短语
3. 说话，识别文本自动输入到光标处（相邻两段字母数字文本之间会自动补空格）

> 快捷键为系统级全局生效。个别以管理员权限运行的前台程序可能收不到注入文本（Windows UIPI 限制），此时请以管理员身份运行 MicYou。

## ⌨️ 自定义快捷键

快捷键**完全自定义**，格式为 `修饰键 + 主键`，用 `+` 连接，例如：

| 示例 | 含义 |
| --- | --- |
| `lctrl+lshift` | 左 Ctrl + 左 Shift（默认，IME 风格的纯修饰键组合） |
| `ctrl+shift` | 任意一侧的 Ctrl + Shift |
| `ctrl+shift+f8` | 经典三键组合（主键会被吞掉，不会传给前台应用） |
| `alt+space` | Alt + 空格 |
| `f13` | 单独 F13（无修饰键时仅支持 F1–F24 与鼠标侧键） |
| `mouse4` | 单独鼠标侧键（后退键；侧键击键会被吞掉，不会触发浏览器前进/后退） |
| `ctrl+mouse5` | Ctrl + 鼠标前进侧键 |

**词法**：

- 修饰键：`ctrl` / `alt` / `shift` / `win`（左右均可触发），或 `lctrl` `rctrl` `lalt` `ralt` `lshift` `rshift` `lwin` `rwin`（区分左右）
- 键盘主键：`a`-`z`、`0`-`9`、`f1`-`f24`、`space`、`enter`、`tab`、`esc`、`backspace`、`delete`、`insert`、`home`、`end`、`pageup`、`pagedown`、`up`/`down`/`left`/`right`、`minus`、`equal`、`bracketleft`、`bracketright`、`backslash`、`semicolon`、`quote`、`comma`、`period`、`slash`、`backquote`、`pause`
- 鼠标侧键：`mouse4`（后退）/ `mouse5`（前进），别名 `xbutton1`/`xbutton2`、`x1`/`x2`、`back`/`forward`；可单独使用或与修饰键组合（如 `ctrl+mouse5`）

**安全规则**（不满足会被拒绝并写入插件日志，沿用旧组合）：

- 字母 / 数字 / 普通键作主键时**必须搭配至少一个修饰键**（否则会劫持正常打字）
- 纯修饰键组合**至少两个修饰键**（单独的 `lctrl` 会让每次 Ctrl+C 都触发）
- 无修饰键的单独主键仅支持 `F1`–`F24` 与鼠标侧键 `mouse4`/`mouse5`（两者都不产生文本输入）
- 鼠标滚轮、左/中/右键不支持作为主键

**两种修改方式，均立即生效**（宿主保存配置时广播 `config:changed`，插件热更新组合，旧组合自动失效，无需禁用再启用）：

1. 插件卡片的配置表单 / JSON 编辑器，改 `hotkey` 字段
2. **专属设置页**（设置对话框侧边栏 → Voice-Typing）：点「按键捕获」，按下组合键或直接点击鼠标侧键，预览为粘性快照（松开按键不丢失），确认后点「使用此组合」——纯修饰键组合与侧键都能直接录入


## 🔊 提示音音量

`volume` 配置（0–100，默认 100，0 = 静音）控制开始/结束提示音的音量，可在配置表单或专属设置页的滑杆调节，**保存后立即生效**。

## 🛠️ 开发者指南

### 编译环境

- Rust（stable，MSVC 工具链）
- Windows 操作系统（深度依赖 Win32 API 与 UI Automation；CI 使用 `RUSTFLAGS="-C target-feature=+crt-static"` 静态链接 CRT）

### 构建步骤

```bash
git clone https://github.com/OrientCOMPASS/MicYou-Voice-Typing.git
cd MicYou-Voice-Typing

cargo test --release    # 快捷键解析、音量缩放等单元测试
cargo build --release   # 产物 target/release/micyou_voice_typing.dll
```

### CI / 发布（`.github/workflows/release.yml`）

- **push 到 main**：自动构建 + 单测，产出可直接安装的 development artifact（`voice-typing-development`），不创建 release / tag
- **手动发 release**：Actions 页面 → Run workflow，可选 bump 档位（none/patch/minor/major）；流程为 构建 → 打包 `opss.voice-typing.zip`（固定资产名）→ 打 tag `v<ver>` → 创建 Release（资产：zip + `plugin.json`）→ 若 bump 则把新版本号回提交 main（带 `[skip ci]`）
- 同版本重复发布会直接失败，防止误覆盖；提交信息含 `[skip ci]` 则整个 workflow 跳过


欢迎在 [Issues](https://github.com/OrientCOMPASS/MicYou-Voice-Typing/issues) 交流具体需求和方案！

## 📄 许可协议

本项目基于 [Unlicense](LICENSE) 协议开源，您可以自由地使用、修改和分发本代码。
