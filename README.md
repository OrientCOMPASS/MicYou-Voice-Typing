# Voice-Typing

> 为 [MicYou 桌面端（Windows）](https://github.com/LanRhyme/MicYou)打造的语音输入插件，让你“说话即打字”。

## 📖 简介

**Voice-Typing** 是一款基于 [WhatdidIsay (WDIS)](https://github.com/OrientCOMPASS/WhatdidIsay) 全局语音转录广播的智能输入插件。

它通过本地维护的**抗延迟时间窗口状态机**与**系统级 UI Automation 光标追踪**，智能过滤 WDIS 的广播数据，将识别到的文本自动注入到当前焦点的输入框中。支持“按键切换 (Toggle)”与“长按说话 (Push-to-Talk)”两种交互模式，可在插件设置面板中无缝切换。

## 📦 依赖要求

本插件**必须**与 [WhatdidIsay (WDIS)](https://github.com/OrientCOMPASS/WhatdidIsay) 插件协同工作。
- WDIS 负责在后台持续进行离线语音识别并广播结果。
- Voice-Typing 负责拦截快捷键，并决定何时将广播转化为键盘输入。

## 🚀 使用说明

1. **安装依赖**：确保已在 MicYou 中安装并启用 `WhatdidIsay` 插件。
2. **激活输入**：默认快捷键为 **左 Ctrl + 左 Shift (LCtrl + LShift)**。
   - **按键切换模式 (默认)**：按一次激活输入（光标旁出现红色麦克风图标），再按一次关闭。适合连续长句输入。
   - **长按说话模式**：按住快捷键期间允许输入，松开即停止。适合碎片化短语输入。
3. **个性化设置**：在 MicYou 的插件设置侧边栏中，可随时切换上述两种触发模式。

## 🛠️ 开发者指南

### 编译环境
- Rust (stable)
- Windows 操作系统 (深度依赖 Win32 API 与 UI Automation)

### 构建步骤
```bash
# 克隆仓库
git clone https://github.com/OrientCOMPASS/MicYou-Voice-Typing.git
cd MicYou-Voice-Typing

# 编译 Release 版本
cargo build --release
```

## 📝 TODO

- [ ] 支持自定义快捷键（目前固定为 `LCtrl + LShift`）
- [ ] 支持调整提示音音量

欢迎在 [Issues](https://github.com/OrientCOMPASS/MicYou-Voice-Typing/issues) 交流具体需求和方案！

## 📄 许可协议

本项目基于 [Unlicense](LICENSE) 协议开源，您可以自由地使用、修改和分发本代码。