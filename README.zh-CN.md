<p align="center">
  <img src="assets/readme/logo.svg" width="112" height="112" alt="Lumora 应用图标">
</p>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/readme/hero-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="assets/readme/hero-light.svg">
    <img src="assets/readme/hero-light.svg" width="100%" alt="Lumora — Capture, beautifully. macOS 桌面上的 Lumora 区域选择界面：变暗的背景、清晰的选区、尺寸标签、像素放大镜和快速访问卡片。">
  </picture>
</p>

<p align="center">
  <b>截图与录屏，用起来就像 macOS 自带的一样。</b><br>
  一款原生菜单栏应用，用 Rust 构建于 AppKit、Core Animation 与 ScreenCaptureKit 之上。
</p>

<p align="center">
  <a href="https://github.com/lyinet/lumora/actions/workflows/ci.yml"><img src="https://github.com/lyinet/lumora/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-0A84FF?style=flat-square" alt="许可证：MIT"></a>
  <img src="https://img.shields.io/badge/macOS-14%2B-111111?style=flat-square&logo=apple&logoColor=white" alt="平台：macOS 14+">
  <img src="https://img.shields.io/badge/Rust-1.80%2B-DEA584?style=flat-square&logo=rust&logoColor=white" alt="Rust 1.80+">
  <img src="https://img.shields.io/badge/status-early%20alpha-FF9F0A?style=flat-square" alt="状态：早期 alpha">
</p>

<p align="center">
  <a href="#功能">功能</a> ·
  <a href="#快捷键">快捷键</a> ·
  <a href="#安装">安装</a> ·
  <a href="#架构">架构</a> ·
  <a href="#路线图">路线图</a> ·
  <a href="README.md">English</a>
</p>

<br>

> [!NOTE]
> **Lumora 目前处于早期 alpha 阶段（v0.7.0）。** 整个应用已能针对 Apple Silicon 和 Intel 通过类型检查与 clippy，与平台无关的核心逻辑也有单元测试覆盖。真机测试刚刚开始（v0.6.1–v0.6.3 修复了它发现的首次启动权限、启动、点击波纹与滚动截图问题；v0.7.0 新增滚轮驱动的滚动截图、适配键盘布局的按键提示与字幕预览，尚未在真机上验证），难免会有粗糙之处。⭐ **点个 Star 关注进展**；哪些已验证、哪些尚未验证，请看 [STATUS.md](STATUS.md)。

## 功能

<table>
  <tr>
    <td width="50%" valign="top">
      <img src="assets/readme/feature-area.svg" width="100%" alt="区域截图：鼠标悬停的窗口以蓝色高亮，单击即可截取">
      <h3>区域截图与窗口吸附</h3>
      <p>选区时屏幕画面会被冻结，视频和动画不会改变你截到的内容。背景变暗、选区清晰，实时显示 <code>W × H</code> 尺寸标签，像素放大镜会显示光标下的十六进制颜色。悬停即可高亮窗口，单击就能干净地截下它——连同阴影，即使它被其他窗口遮挡。拖动时，选区边缘会自动吸附到附近的窗口和显示器边缘。</p>
    </td>
    <td width="50%" valign="top">
      <img src="assets/readme/feature-quick-access.svg" width="100%" alt="快速访问：左下角叠放的玻璃卡片，其中一张正被拖进聊天窗口">
      <h3>快速访问</h3>
      <p>每次截图都会落进一张玻璃卡片，借弹簧动画从左下角滑入。悬停可拷贝、保存、钉住或删除，也可以把卡片直接拖进 Slack、邮件或访达。最多叠放三张，平滑重排；不悬停时 6 秒后自动隐藏。</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <img src="assets/readme/feature-pin.svg" width="100%" alt="钉在屏幕上：一张截图悬浮在编辑器窗口之上，并显示缩放比例">
      <h3>钉在屏幕上</h3>
      <p>把任意截图悬浮在所有窗口之上作为参考。滚动缩放，<kbd>⌥</kbd> 加滚动调节透明度，拖动移动，双击或按 <kbd>Esc</kbd> 关闭。</p>
    </td>
    <td width="50%" valign="top">
      <img src="assets/readme/feature-recording.svg" width="100%" alt="录屏：红色流动虚线边框、3-2-1 倒计时圆环和菜单栏中的红色计时器">
      <h3>屏幕录制</h3>
      <p>录制选定区域或全屏，输出 H.264 或 HEVC 编码的 MP4，30 或 60 fps，可显示或隐藏光标，支持系统音频与麦克风（macOS 14+），并可显示点击涟漪。3-2-1 倒计时圆环、菜单栏里的红色计时器、悬浮的停止按钮，以及一圈绝不会出现在视频里的流动虚线边框。</p>
    </td>
  </tr>
</table>

**还有那些让它“感觉对了”的细节：**

- **Retina 精确输出** —— 按原生像素密度保存 PNG 并写入 DPI 元数据，在任何地方都以正确尺寸显示。文件名形如 `Lumora 2026-11-15 at 04.30.12.png`，并以 PNG 格式（附带文件本身，可直接粘贴到访达或聊天软件）自动拷贝到剪贴板。
- **键盘级精度** —— 不碰鼠标也能移动选区、锁定比例、逐点微调（见下文）。
- **符合物理的动效** —— 每一次出现、吸附、微调和重排都是一个阻尼弹簧，被打断时会保留速度继续过渡。直接操作从不加动画，选区永远不会落后于你的指针。
- **先快门，再飞入** —— 选区闪一下白光，截图随即缩小飞向卡片，同时卡片滑入。
- **美化** —— 为截图加上渐变、纯色或图片背景，配合留白、圆角、柔和阴影、可选的 macOS 窗口边框和固定比例（16:9、1:1 等），在标注编辑器的 ✨ 面板中实时编辑。
- **可再编辑的 `.lumora` 文档** —— 每张保存的截图旁都会有一个 `.lumora` 文件，保存原始截图、标注和美化样式；在访达中双击（或用“打开…”），每个箭头、文字和裁剪都仍可编辑。
- **标注样式** —— 除了颜色和线宽，编辑器的样式栏还有不透明度滑块（10–100 %），适用于任何标注（打码区域始终完全不透明）。样式会作用于选中的标注，并作为一次可撤销的操作；样式保存在 `.lumora` 文档中，导出效果与编辑器中完全一致。
- **AI 写作（可选，默认关闭）** —— 在编辑器中点 ✨ 或按 <kbd>⌘</kbd><kbd>J</kbd>，即可生成配图说明、步骤指南、替代文本，或进行润色、翻译（English / 简体中文）、总结，结果流式显示，可一键插入为文字标注。服务商自选：OpenAI、DeepSeek、通义千问、Kimi（月之暗面）、本地 Ollama 或任意 OpenAI 兼容接口。API 密钥保存在 macOS 钥匙串中；默认只发送在本机识别出的文字和你的标注文字——已裁掉或已打码区域内的文字不会发送，只有在你主动开启后才会发送截图本身。每张截图只识别一次文字，智能打码和所有 AI 任务都会复用。
- **智能打码** —— 在编辑器中点一下（或按 <kbd>⌘</kbd><kbd>⇧</kbd><kbd>R</kbd>），即可对邮箱、手机号、身份证号与银行卡号（经校验位验证）、API 密钥与令牌、IP 地址、链接中的令牌以及人脸进行马赛克或模糊处理——全部用 Vision 在本机检测，并作为一次可撤销的操作添加。可在“设置 → 截图”中选择检测类别，也可开启“复制 / 保存新截图前自动打码”，或启用只发送识别文字、绝不发送图片的 AI 检查。
- **文字识别操作** —— “截取文字”（<kbd>⌥</kbd><kbd>⇧</kbd><kbd>2</kbd>）拷贝文字后，会弹出一个小浮动面板，提供拷贝、总结、翻译（English / 简体中文）、转为表格、转为代码和待办清单，结果流式显示在结果面板中，可拷贝、停止和重试。总结与翻译需要开启 AI 写作，其余功能无需 AI 即可使用。
- **截图转表格 / 代码** —— 在本机根据文字位置还原表格的行与列，可拷贝为 Markdown 或 CSV；也能把代码截图还原成代码并恢复缩进。开启 AI 写作后，AI 会在本地结果的基础上进一步优化；编辑器的 AI 菜单中也新增了“提取表格”和“提取代码”。
- **点击涟漪** —— 录屏时，每次点击都会出现一个带弹性的圆环（左键蓝色、右键橙色）并录进视频，而边框、停止按钮和提示浮窗依然不会出现在画面里。可在“设置 → 录屏”中开关。每次点击还会连同时间、位置和所在应用一起记录到视频旁的 `<视频名>.events.json` 文件中，方便后续生成分步教程。
- **按键提示** —— 可选（“设置 → 录屏”，默认关闭）：<kbd>⇧</kbd><kbd>⌘</kbd><kbd>4</kbd> 等快捷键以及 ↩ ⌫ ← 等按键会以小胶囊的形式录进视频（底部居中、左下 / 右下或顶部居中），连按会合并为 `×N`，约一秒后淡出。按键标签跟随当前键盘布局（AZERTY 上标有 W 的键显示为 <kbd>⌘</kbd><kbd>W</kbd>；俄语、希腊语等非拉丁布局下，<kbd>⌘</kbd>/<kbd>⌃</kbd> 快捷键与菜单一致，显示 macOS 实际使用的拉丁字母），录制中切换输入法也会随之更新。普通输入的文字永不显示，密码框开启安全输入时也不显示任何内容，按键从不保存。需要“输入监控”权限；未授权时照常录制，只是不显示按键。
- **录屏转教程** —— 在录屏的快速访问卡片上点“生成教程”（或在菜单栏选择“从录屏生成教程…”），Lumora 会把每次点击变成分步指南：取点击后一刻的画面并画上高亮圆环、围绕指针裁切，再根据指针旁的文字（在本机识别）生成标题。开启 AI 后，步骤由你的服务商撰写，只发送文字——应用名、窗口标题和附近的文字，绝不发送画面。可以编辑或删除步骤，然后导出 Markdown、单文件自包含 HTML（支持浅色与深色），或直接拷贝 Markdown。
- **剪辑与 GIF** —— 在录屏的快速访问卡片或截图历史中点“剪辑 / GIF…”（剪刀图标）。拖动“开始”和“结束”（预览随之更新），然后点“保存剪辑”——已保存的录屏会在旁边生成 `<文件名> (trimmed)` 副本，临时录屏则直接原地剪辑；点击记录和字幕会同步平移（教程步骤不保留）。“导出 GIF…”使用同一范围，可选 480 / 640 / 800 像素或原始宽度、10 / 15 / 20 帧/秒，并显示预估大小，GIF 无限循环，保存在录屏旁边。两者都在后台进行，带进度条和“取消”；剪辑不会重新编码视频。
- **字幕、摘要与章节** —— 在录屏的快速访问卡片上点“字幕”（或在菜单栏选择“转写录屏…”）。默认在本机用 Apple 语音识别（除非你允许，否则绝不使用 Apple 服务器），也可以改用 AI 服务商的 Whisper 兼容接口（音频不超过 25 MB）。在“设置 → 录制”中选择语音语言（中文、英语或跟随应用语言）和音频来源（麦克风、系统声音或两者混合）。随后 Lumora 生成简短摘要和章节——开启 AI 时由 AI 撰写（仅发送文字），否则根据停顿和点击在本机生成。可以逐句修改（按 <kbd>Esc</kbd> 结束编辑，再按一次关闭窗口；长录屏也能立即打开），然后导出 SRT 或 WebVTT，或拷贝章节（YouTube 格式）和摘要。点“烧录字幕…”可把字幕直接画进视频，生成 `<文件名> (subtitled)` 副本（白色文字，字号小 / 中 / 大，位于底部或顶部，可加圆角背景框，设置会被记住），在后台进行并带进度条和“取消”，完成后出现在快速访问中。字幕列表上方有实时预览：显示视频画面，并按烧录时完全相同的方式绘制字幕（针对该视频尺寸的字号、换行、背景框、边距与位置都一致）；它会跟随样式选项和你的修改，点进某一句时跳到那一句，并可用滑块在整段视频中拖动查看。纯音频录音则显示一行提示。所有内容保存在 `<视频名>.transcript.json` 中，再次打开时保留你的修改。
- **滚动截图** —— 按 <kbd>⌥</kbd><kbd>⇧</kbd><kbd>S</kbd>（或在菜单栏选择“滚动截图”），框选窗口中会滚动的区域，然后缓慢滚动——向下，或向上（例如打开时停在最新消息的聊天页面）。Lumora 会随着你的滚动截取画面——依据鼠标滚轮和触控板在该区域上的滚动事件（滚动时每秒最多约八帧，页面停稳或触控板惯性滚动结束时再补一帧；在屏幕其他位置滚动会被忽略），对于不产生滚轮事件的滚动（方向键 / 翻页键、拖动滚动条）则由定时器兜底——再把有移动的画面拼接成一张长图。滚动的方向和距离还能帮助把重复的内容（一模一样的列表行）对齐到正确位置：顶部和底部固定不动的栏（浏览器工具栏、导航栏、聊天输入栏）只保留一份，滚动区域旁固定的侧边栏不会干扰拼接，细小、平滑或惯性滚动的步进也能对齐。小面板放在截取区域之外，不会出现在图里，它会显示已截取的帧数——点“完成”（或按 <kbd>↩</kbd>、再按一次快捷键）结束，点“取消”（或按 <kbd>Esc</kbd>）放弃。开始滚动之前不会自动结束：点击窗口使其获得焦点、悬停效果、闪烁的光标或没有移动的画面都会被跳过。滚回已拼接过的部分不会重复添加；开始滚动后，若约两秒内都无法衔接（滚动太快）或高度达到 20,000 像素，会自动停止并提示，已拼接的部分会保留。Lumora 不会替你滚动，只是旁观滚轮事件（使用 AppKit 事件监视器，监视滚动事件不需要任何权限），因此无需辅助功能或输入监控权限。结果和普通截图一样：快速访问、复制 / 保存、截图历史和编辑器。
- **截图历史** —— 按 <kbd>⌥</kbd><kbd>⇧</kbd><kbd>H</kbd>（或在菜单栏选择“截图历史…”），以缩略图网格浏览截图文件夹中的所有截图、录屏和 `.lumora` 文档。可按类型筛选，并搜索文件名*以及截图里的文字*（窗口打开时在本机识别），然后在编辑器中重新打开（有 `.lumora` 时优先打开它，标注仍可编辑）、拷贝、在访达中显示或移到废纸篓。只会列出 Lumora 自己的文件（依据 `.lumora` / 附属文件或你的文件名模板），缩略图在后台生成并缓存。可选的保留期限设置（设置 → 通用：全部保留、30 天或 90 天）只会*提示*将较旧的项目移到废纸篓，并需要确认——绝不会直接删除任何文件。
- **macOS 14+ 麦克风录制** —— 在 macOS 14 和 15 上都能边录屏边录下你的声音，可使用系统默认麦克风，或在“设置 → 录屏”中选择设备。首次使用时 Lumora 会请求麦克风权限。

## 快捷键

使用 Option-Shift 组合，避免与 macOS 自带的 <kbd>⇧</kbd><kbd>⌘</kbd><kbd>3</kbd>/<kbd>4</kbd>/<kbd>5</kbd> 冲突。所有快捷键都可在 **设置 → 快捷键** 中重新绑定。

| 全局 | 快捷键 |
|---|---|
| 区域截图 | <kbd>⌥</kbd> <kbd>⇧</kbd> <kbd>4</kbd> |
| 全屏截图 | <kbd>⌥</kbd> <kbd>⇧</kbd> <kbd>3</kbd> |
| 窗口截图 | <kbd>⌥</kbd> <kbd>⇧</kbd> <kbd>5</kbd> |
| 录屏（开始 / 停止） | <kbd>⌥</kbd> <kbd>⇧</kbd> <kbd>R</kbd> |
| 打开截图文件夹 | <kbd>⌥</kbd> <kbd>⇧</kbd> <kbd>O</kbd> |
| 截图历史 | <kbd>⌥</kbd> <kbd>⇧</kbd> <kbd>H</kbd> |
| 滚动截图（开始 / 完成） | <kbd>⌥</kbd> <kbd>⇧</kbd> <kbd>S</kbd> |

| 截图界面内 | 按键 |
|---|---|
| 拖动时移动整个选区 | <kbd>Space</kbd> |
| 锁定宽高比 | <kbd>⇧</kbd> |
| 微调 1 pt · 10 pt · 调整大小 | <kbd>←</kbd> <kbd>↑</kbd> <kbd>→</kbd> <kbd>↓</kbd> · 加 <kbd>⇧</kbd> · 加 <kbd>⌥</kbd> |
| 临时关闭边缘吸附 | 拖动时按住 <kbd>⌥</kbd> |
| 拷贝放大镜下的颜色（十六进制） | <kbd>C</kbd> |
| 确认（或双击） | <kbd>Enter</kbd> |
| 取消 | <kbd>Esc</kbd> |

## 为什么选择 Lumora

<table>
  <tr>
    <td width="50%" valign="top"><b>彻头彻尾的原生</b><br>Rust 通过类型化的 <code>objc2</code> 绑定直接调用 AppKit、Core Animation 和 ScreenCaptureKit。没有 WebView，没有 Electron，也无需附带运行时。</td>
    <td width="50%" valign="top"><b>闭式解的弹簧物理</b><br>动效来自 <code>lumora-core</code> 中的阻尼振子解析解，Core Animation 图层与窗口动画共用同一模型，稳定时间和过冲量都由单元测试约束。</td>
  </tr>
  <tr>
    <td width="50%" valign="top"><b>小巧的原生二进制</b><br>一款编译为单个原生可执行文件的菜单栏应用，发布版启用 thin LTO 并剥离符号。</td>
    <td width="50%" valign="top"><b>开源，MIT 许可</b><br>代码清晰、文档完善：严格的 crate 分层，每个 unsafe 代码块都有 <code>SAFETY:</code> 注释，设计令牌同步记录在 <a href="DESIGN.md">DESIGN.md</a> 中。</td>
  </tr>
</table>

## 安装

**Homebrew cask —— 即将推出。** 签名并公证的版本会在真机调试完成后发布（见[路线图](#路线图)）。在此之前，可以从 Releases 页面最新的预发布版本下载 `Lumora-<版本>.dmg`（ad-hoc 签名：把 Lumora 拖到“应用程序”，首次启动请看[故障排除](#故障排除)），或从源码构建。

### 从源码构建

环境要求：macOS 14+、Xcode 命令行工具、Rust stable 1.80+。

```sh
git clone https://github.com/lyinet/lumora.git
cd lumora

# 在终端中运行调试版本
#（权限请求中显示的将是你的终端应用）
cargo run -p lumora

# 通用（arm64 + x86_64）应用包 + DMG，ad-hoc 签名
brew install create-dmg   # 可选；未安装时回退到 hdiutil
scripts/bundle.sh
open dist/Lumora.app

# 签名并公证的发布版本
SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
NOTARY_PROFILE=lumora-notary scripts/bundle.sh
```

> [!TIP]
> “屏幕录制”权限与应用的代码签名绑定。ad-hoc 构建每次都会得到新的签名身份，所以每次重新构建后 macOS 都会再次请求授权——日常开发请使用固定的签名身份。

### 签名版本

CI 可以用 Developer ID 证书为 Lumora 签名并提交 Apple 公证：这样更新后 macOS 会保留“屏幕录制”“麦克风”等权限，打开应用时也不会出现 Gatekeeper 警告。只要仓库里配置了签名密钥（`MACOS_CERT_P12`、`MACOS_CERT_PASSWORD`、`APPLE_TEAM_ID`，以及用于公证的 App Store Connect API 密钥或 Apple ID 专用密码），它就会自动启用；没有这些密钥时，CI 仍和以前一样构建 ad-hoc 签名的 DMG。需要付费的 Apple 开发者账号。配置方法见 [docs/signing.md](docs/signing.md)（英文）。

### 权限

Lumora 只在某项功能第一次需要时才请求对应权限：

| 权限 | 用途 |
|---|---|
| **屏幕录制** | 所有截图与录屏（必需） |
| **麦克风** | 录下你的声音，仅在“设置 → 录屏”中开启麦克风时 |
| **语音识别** | 用 Apple 语音在本机生成字幕，仅在你转写录屏时 |
| **输入监控** | 录屏时的按键提示，仅在“设置 → 录屏”中开启时（只监听，不保存任何内容） |
| **钥匙串** | 保存和读取 AI 服务商的 API 密钥（`app.lumora.ai`），仅在使用 AI 功能时 |

无需“辅助功能”权限；“输入监控”仅用于可选的按键提示。网络请求只会发往你配置的 AI 服务商，且仅在你使用 AI 功能或云端转写时发生。

### 故障排除

Lumora 是一个**菜单栏应用**：没有程序坞图标，也没有主窗口。如果启动时已经允许了“屏幕录制”，它会显示一条“Lumora 正在菜单栏中运行”的提示，之后就待在菜单栏图标里。在“访达”或“聚焦”中再次打开 Lumora 会打开设置窗口。

**“屏幕与系统录音”列表里没有 Lumora，或点击“授予权限”没有反应**

1. 把 `Lumora.app` 移到 `/Applications`（从磁盘映像中拖过去），并从那里打开。直接从磁盘映像或“下载”文件夹打开的副本会在临时位置运行（App Translocation），权限不会保留在它上面。Lumora 发现这种情况时会主动提出把自己移过去。
2. 如果 macOS 仍然不显示它，移除下载隔离标记后再打开：
   ```sh
   /usr/bin/xattr -dr com.apple.quarantine /Applications/Lumora.app
   ```
3. 重置 Lumora 的屏幕录制记录，打开 Lumora，点击 **授予权限** 并允许，然后重新启动：
   ```sh
   tccutil reset ScreenCapture com.lumora.app
   ```

**截图没有反应，或已经打开开关但 macOS 仍然拒绝。** 预发布版本使用 ad-hoc 签名，macOS 会把**每次更新都当作一个新应用**，旧的授权不再生效。安装更新后，请在“屏幕与系统录音”中用“–”删除 Lumora（或运行上面的 `tccutil` 命令），重新允许并重新启动。现在 macOS 拒绝截图时，Lumora 会自己给出这些说明。

**授予权限时，请从“访达”、“聚焦”或“启动台”打开 Lumora，而不是从终端启动。** 从终端启动时（直接运行可执行文件，某些环境下甚至是 `open`），macOS 会把终端视为负责的应用：弹出的提示会请求让*你的终端*（终端、iTerm、XTerminal 等）录制屏幕，权限也会授予终端而不是 Lumora。请退出 Lumora，在“应用程序”中双击打开它（或通过“聚焦”/“启动台”），再点击 **授予权限**。

**菜单栏里看不到图标。** 在 macOS 26 上请检查“系统设置”→“菜单栏”并允许 Lumora；菜单栏太拥挤时（尤其是刘海附近）图标可能被隐藏。快捷键（<kbd>⌥</kbd><kbd>⇧</kbd><kbd>4</kbd> 等）仍然可用。

**日志。** Lumora 会把日志写到 `~/Library/Logs/Lumora/lumora.log`（也可在“控制台”App 中查看）；崩溃时的 panic 信息和调用栈也会记录在这里。需要更多细节时，用调试日志启动：
```sh
LUMORA_DEBUG=1 /Applications/Lumora.app/Contents/MacOS/lumora   # 或 LUMORA_LOG=debug / RUST_LOG=debug
```
注意：从终端启动的副本使用的是**终端**的“屏幕录制”权限（见上文），因此这种方式只适合查看日志，不适合授予或测试权限；无论哪种方式日志文件都会写入，所以遇到权限问题时请正常启动，之后再查看 `~/Library/Logs/Lumora/lumora.log`。报告问题时请附上这份日志。

**滚动截图拼接不起来。** 每次滚动截图结束时，日志里都有一行 `scrolling capture summary:`（追加 / 前插 / 未变化 / 重新开始 / 无法衔接的帧数、识别到的固定页眉页脚、图片尺寸），前几帧无法衔接的画面也会记下匹配情况；另有一行 `scrolling capture frames:`，统计由滚动事件触发和由定时器触发的帧数，以及区域内 / 外收到的滚轮事件数。开启调试日志（见上文）后，每一帧都会记录：找到的偏移、固定区域、固定列以及处理结果。报告问题时请附上这一段日志。

<details>
<summary><b>在 Linux 或 CI 中开发</b></summary>

<br>

纯逻辑的 crate 可以在任何平台上构建和测试：

```sh
cargo test -p lumora-core -p lumora-ui -p lumora-ai   # 几何、弹簧、选区、设置、快捷键、设计令牌
```

安装 Apple 目标后，可以在任何主机上对整个应用做类型检查：

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo check --workspace --target aarch64-apple-darwin
cargo clippy --workspace --all-targets --target x86_64-apple-darwin -- -D warnings
```

</details>

<details>
<summary><b>设置与日志</b></summary>

<br>

设置以 TOML 格式保存在 `~/Library/Application Support/Lumora/settings.toml`。缺失的键使用默认值，未知的键会被忽略，损坏的文件会被移到 `settings.toml.bak`。清除的快捷键会保存为 `""`，重启后依然保持清除状态。

截图历史索引（文件列表、尺寸以及截图中识别出的文字，用于搜索）保存在同目录的 `history.json`，缩略图在 `Thumbnails/` 中。两者都只是缓存：随时可以删除，下次扫描会自动重建；所有内容都不会离开你的 Mac。日志输出到 stderr 和 `~/Library/Logs/Lumora/lumora.log`；设置 `LUMORA_DEBUG=1`（或 `LUMORA_LOG=debug` / `RUST_LOG=debug`）可查看更多信息——见[故障排除](#故障排除)。

</details>

## 架构

<p align="center">
  <img src="assets/readme/architecture.svg" width="720" alt="五个 crate，依赖方向自上而下：最上层是 apps/lumora，其下是 lumora-ui、lumora-ai 与 lumora-capture，它们都依赖 lumora-core">
</p>

项目是一个由五个 crate 组成的 Cargo 工作区，依赖方向严格单向。不依赖操作系统的逻辑都放在最底层的 crate 中，在任何主机上都能测试；其上的每一层都是对 Apple 框架的一层轻薄、类型化的封装。

| Crate | 职责 |
|---|---|
| [`apps/lumora`](apps/lumora) | 可执行程序：`NSApplication`、状态栏图标、隐藏的主菜单、Carbon 全局快捷键、应用状态，以及截图 / 录屏 / 文字识别 / 打码 / 教程 / 转写流程 |
| [`lumora-ui`](crates/lumora-ui) | AppKit + Core Animation：截图界面、快速访问、钉图、快门、倒计时、录屏控件、设置、引导、标注编辑器 + 美化面板 + AI 结果面板、识别文字操作面板、点击涟漪浮层、教程、字幕与截图历史窗口 |
| [`lumora-ai`](crates/lumora-ai) | OpenAI 兼容的流式对话与 `/audio/transcriptions` 客户端、钥匙串密钥存储 |
| [`lumora-capture`](crates/lumora-capture) | ScreenCaptureKit、ImageIO、AVFoundation（含 macOS 14 上的 `AVCaptureSession` 麦克风）、剪贴板、美化渲染、Vision 文字识别与人脸检测、教程帧渲染、Apple 语音转写 |
| [`lumora-core`](crates/lumora-core) | 纯 Rust：几何、弹簧、选区状态机、吸附、设置、文件名、快捷键解析、布局、录屏状态机、美化模型、`.lumora` 文档格式、AI 设置/提示词/流式解析、打码检测器、表格 / 代码提取、点击记录、教程模型与导出、字幕（SRT / VTT、章节、摘要、烧录排版）、临时文件夹清理、截图历史索引 |

深入了解：[ARCHITECTURE.md](ARCHITECTURE.md)（线程模型、数据流、坐标空间）· [DESIGN.md](DESIGN.md)（颜色、形状、字体与动效令牌）。

## 路线图

以下内容均已编写完成、通过类型检查和单元测试，但**尚未在真机上运行**。

- [x] **v0.1 —— 截图与录屏：** 在冻结画面上的区域/窗口/全屏截图、快速访问、钉图、屏幕录制、设置、引导
- [x] **v0.2 —— 标注：** 标注编辑器、OCR（截取文字）、提示浮窗、简体中文本地化、应用图标、仅窗口模式、跨显示器选区
- [x] **v0.3 —— 美化：** 背景、留白、阴影、窗口边框、固定比例；可再编辑的 `.lumora` 文档
- [x] **v0.4 —— AI 写作：** 使用你自己的服务商生成说明、步骤指南、替代文本，以及润色、翻译、总结
- [x] **v0.5 —— AI 组合：** 智能打码（本机检测、一键马赛克 / 模糊、可选 AI 检查）；文字识别操作面板与截图转表格 / 代码，本地优先；点击涟漪、点击事件记录与 macOS 14 麦克风录制；录屏转分步教程；字幕、摘要与章节（本机或云端识别，导出 SRT / VTT）
- [x] **v0.5.1 —— 打磨：** 剪贴板改用 PNG；每张截图只识别一次文字（AI 写作不再读取已裁掉或已打码的文字）；钥匙串读取不再阻塞主线程；字幕窗口更稳健；字幕错误提示更清晰；设置各标签页可滚动

- [x] **v0.6 —— 历史与录屏增强：** 截图历史窗口（<kbd>⌥</kbd><kbd>⇧</kbd><kbd>H</kbd>）、按键提示、剪辑与导出 GIF、字幕烧录、滚动截图（<kbd>⌥</kbd><kbd>⇧</kbd><kbd>S</kbd>）

- [x] **v0.7.0 —— 滚动、按键与字幕：** 由滚轮驱动的滚动截图、按键提示适配键盘布局、字幕烧录实时预览（等待真机验证）

**v0.7 —— 下一步**

- [ ] 在 Apple Silicon 与 Intel（macOS 14 和 15）上真机调试并修复问题
- [ ] 在 CI 中进行 Developer ID 签名与公证——流程已就绪（[docs/signing.md](docs/signing.md)），等待 Apple 开发者账号

**以后**

- [ ] Sparkle 自动更新、Homebrew cask
- [ ] 编辑器打磨、录屏暂停/继续与摄像头气泡、云端链接、快捷指令自动化
- [ ] Mac App Store 版本、更多语言（日、繁中、德、法、西）

完整内容见 [ROADMAP.md](ROADMAP.md) 和 [CHANGELOG.md](CHANGELOG.md)。

## 参与贡献

非常欢迎贡献——而眼下最有价值的贡献，就是**在 Mac 上跑一跑 Lumora**。如果你试用了，请开一个 issue，写明芯片型号、macOS 版本，以及哪些正常、哪些出了问题。

- **较大的改动**请先开 issue 讨论。
- **保持 CI 通过：** `cargo fmt --all`、`cargo clippy --workspace --all-targets -- -D warnings`（在 Mac 上运行，或在其他平台加上 `--target aarch64-apple-darwin`），以及 `cargo test`（核心 crate 可在任何主机上运行）。
- **设计令牌存在于两处** —— `crates/lumora-ui/src/tokens.rs` 和 [DESIGN.md](DESIGN.md)，修改时请同步更新。
- **unsafe 代码**仅限于 FFI 调用，且每处都要有 `SAFETY:` 注释。

## 许可证

[MIT](LICENSE) © Lumora contributors

<br>

<p align="center">
  <img src="assets/readme/logo.svg" width="40" height="40" alt="">
  <br>
  <sub>用 Rust、AppKit 和许多弹簧打造。</sub>
</p>
