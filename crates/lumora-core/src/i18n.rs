//! Localisation: every user-facing string, keyed by [`Msg`], in English and
//! Simplified Chinese.
//!
//! The table is a single macro invocation so adding a key without a
//! translation is a compile error, and the tests below check what the
//! compiler can't (empty strings, mismatched `{0}` placeholders, ellipsis
//! and shortcut glyph consistency).
//!
//! The active language is process-global ([`set_language`]); the app sets
//! it at launch from settings (`general.language`) and the system's
//! preferred languages, and again whenever the setting changes.

use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

/// A UI language Lumora ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Language {
    #[default]
    En,
    ZhHans,
}

impl Language {
    pub const ALL: [Language; 2] = [Language::En, Language::ZhHans];

    /// BCP 47 tag.
    pub fn tag(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::ZhHans => "zh-Hans",
        }
    }

    /// Match one BCP 47 tag (`"zh-Hans-CN"`, `"en_GB"`, `"zh-TW"` …).
    /// Traditional Chinese maps to Simplified, which every Chinese reader
    /// can read; that beats falling back to English.
    pub fn from_tag(tag: &str) -> Option<Language> {
        let primary = tag.split(['-', '_']).next()?.to_ascii_lowercase();
        match primary.as_str() {
            "en" => Some(Language::En),
            "zh" => Some(Language::ZhHans),
            _ => None,
        }
    }

    /// The first supported language in the user's preference order
    /// (`NSLocale.preferredLanguages`), else English.
    pub fn from_preferred<S: AsRef<str>>(preferred: &[S]) -> Language {
        preferred.iter().find_map(|t| Language::from_tag(t.as_ref())).unwrap_or_default()
    }
}

/// The language setting: follow the system, or force one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum LanguagePref {
    #[default]
    #[serde(rename = "system")]
    System,
    #[serde(rename = "en")]
    English,
    #[serde(rename = "zh-Hans")]
    SimplifiedChinese,
}

impl LanguagePref {
    pub const ALL: [LanguagePref; 3] = [LanguagePref::System, LanguagePref::English, LanguagePref::SimplifiedChinese];

    /// Resolve against the system's preferred languages.
    pub fn resolve<S: AsRef<str>>(self, preferred: &[S]) -> Language {
        match self {
            LanguagePref::System => Language::from_preferred(preferred),
            LanguagePref::English => Language::En,
            LanguagePref::SimplifiedChinese => Language::ZhHans,
        }
    }

    /// Label in the Settings popup. Language names are shown in their own
    /// language so a user stuck in the wrong one can find theirs.
    pub fn label(self) -> &'static str {
        match self {
            LanguagePref::System => tr(Msg::LanguageSystem),
            LanguagePref::English => "English",
            LanguagePref::SimplifiedChinese => "简体中文",
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Set the process-wide UI language.
pub fn set_language(lang: Language) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

/// The process-wide UI language.
pub fn language() -> Language {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Language::ZhHans,
        _ => Language::En,
    }
}

/// The string for `msg` in the current language.
pub fn tr(msg: Msg) -> &'static str {
    msg.text(language())
}

/// [`tr`] with `{0}`, `{1}`, … replaced by `args`.
pub fn trf(msg: Msg, args: &[&dyn Display]) -> String {
    format_args_into(tr(msg), args)
}

/// Replace `{n}` placeholders in `template`.
pub fn format_args_into(template: &str, args: &[&dyn Display]) -> String {
    let mut out = template.to_string();
    for (i, a) in args.iter().enumerate() {
        out = out.replace(&format!("{{{i}}}"), &a.to_string());
    }
    out
}

macro_rules! messages {
    ($($key:ident => $en:expr, $zh:expr;)*) => {
        /// A user-facing string.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Msg { $($key),* }

        impl Msg {
            /// Every key, for exhaustive tests.
            pub const ALL: &'static [Msg] = &[$(Msg::$key),*];

            /// The string in `lang`.
            pub fn text(self, lang: Language) -> &'static str {
                match lang {
                    Language::En => match self { $(Msg::$key => $en),* },
                    Language::ZhHans => match self { $(Msg::$key => $zh),* },
                }
            }
        }
    };
}

messages! {
    // -- app & menu ---------------------------------------------------------------
    AppName => "Lumora", "Lumora";
    CaptureArea => "Capture Area", "截取区域";
    CaptureFullscreen => "Capture Fullscreen", "截取全屏";
    CaptureWindow => "Capture Window", "截取窗口";
    CaptureText => "Capture Text", "识别文字";
    RecordScreen => "Record Screen", "录制屏幕";
    StopRecording => "Stop Recording", "停止录制";
    OpenCapturesFolder => "Open Captures Folder", "打开截图文件夹";
    CaptureHistoryMenu => "Capture History…", "截图历史…";
    OpenDocumentMenu => "Open…", "打开…";
    SettingsMenu => "Settings…", "设置…";
    PermissionMenu => "Screen Recording Permission…", "屏幕录制权限…";
    QuitLumora => "Quit Lumora", "退出 Lumora";

    // -- overlay ------------------------------------------------------------------
    HintCapture => "Drag to select  ·  Click a window  ·  Space move  ·  ⇧ lock ratio  ·  C copy colour  ·  Esc",
        "拖动选择  ·  点击窗口  ·  空格移动  ·  ⇧ 锁定比例  ·  C 复制颜色  ·  Esc";
    HintWindow => "Click a window to capture it  ·  Esc to cancel", "点击要截取的窗口  ·  Esc 取消";
    HintText => "Drag over the text to copy  ·  Esc to cancel", "拖动框选要复制的文字  ·  Esc 取消";
    HintRecord => "Drag to choose an area to record  ·  Click for full screen  ·  Esc to cancel",
        "拖动选择录制区域  ·  点击录制全屏  ·  Esc 取消";
    CopiedHex => "Copied {0}", "已复制 {0}";

    // -- quick access -------------------------------------------------------------
    Copy => "Copy", "拷贝";
    Save => "Save", "保存";
    Annotate => "Annotate", "标注";
    PinToScreen => "Pin to Screen", "钉在屏幕上";
    Delete => "Delete", "删除";

    // -- recording ----------------------------------------------------------------
    Stop => "Stop", "停止";
    StopRecordingTip => "Stop Recording ({0})", "停止录制（{0}）";

    // -- onboarding ---------------------------------------------------------------
    WelcomeTitle => "Welcome to Lumora", "欢迎使用 Lumora";
    OnboardingAsk => "Lumora captures your screen only when you ask it to — a shortcut, the menu bar, or a recording you start. macOS requires you to allow Screen Recording once.\n\nClick Grant Access, then switch Lumora on in System Settings.",
        "Lumora 只会在你需要时截取屏幕——按下快捷键、使用菜单栏或开始录制时。macOS 需要你授予一次“屏幕录制”权限。\n\n点击“授予权限”，然后在“系统设置”中打开 Lumora。";
    OnboardingDone => "Screen Recording is allowed. Relaunch Lumora so macOS applies it, then press {0} to capture an area.",
        "已允许屏幕录制。请重新启动 Lumora 使其生效，然后按 {0} 截取区域。";
    GrantAccess => "Grant Access", "授予权限";
    OpenSystemSettings => "Open System Settings", "打开系统设置";
    Relaunch => "Relaunch", "重新启动";
    NotNow => "Not Now", "以后再说";

    // -- launch feedback (v0.6.1) ------------------------------------------------
    ToastRunningTitle => "Lumora is running in the menu bar", "Lumora 正在菜单栏中运行";
    ToastRunningNoShortcuts => "Click the Lumora icon in the menu bar to capture.", "点击菜单栏中的 Lumora 图标即可截图。";
    ToastMenuBarHiddenTitle => "Lumora is running, but its menu bar icon is hidden", "Lumora 正在运行，但菜单栏图标被隐藏了";
    ToastMenuBarHiddenDetail => "Allow Lumora in System Settings → Menu Bar, or use the shortcuts.", "请在“系统设置”→“菜单栏”中允许 Lumora，或直接使用快捷键。";

    // -- capture diagnostics (v0.6.1) --------------------------------------------
    ToastBusy => "Lumora is still busy with another capture", "Lumora 仍在进行另一个截图任务";
    ToastBusyDetail => "Finish or cancel it first (Esc), then try again.", "请先完成或取消它（Esc），然后再试。";
    ToastCaptureStalled => "Screen capture didn’t respond", "屏幕捕获没有响应";
    ToastCaptureStalledDetail => "If this keeps happening, reset the Screen Recording permission.", "如果反复出现，请重置“屏幕录制”权限。";
    ToastPermissionMissing => "Screen Recording permission is needed", "需要“屏幕录制”权限";
    ToastPermissionMissingDetail => "Allow Lumora in the window that just opened.", "请在刚打开的窗口中允许 Lumora。";
    ResetPermissionTitle => "macOS blocked the screen capture", "macOS 阻止了屏幕捕获";
    ResetPermissionBody => "Lumora shows as allowed, but macOS still refuses to capture. This happens when the permission belongs to an older build of Lumora (each unsigned update counts as a new app).\n\nTo fix it: in System Settings → Privacy & Security → Screen & System Audio Recording, select Lumora and remove it with “–”, then add it again with “+” (or run tccutil reset ScreenCapture com.lumora.app in Terminal), and relaunch Lumora.",
        "Lumora 显示为已允许，但 macOS 仍拒绝捕获屏幕。这通常是因为该权限属于旧版本的 Lumora（每次未签名的更新都会被视为新应用）。\n\n解决方法：在“系统设置”→“隐私与安全性”→“屏幕与系统录音”中选中 Lumora，用“–”删除，再用“+”重新添加（或在“终端”中运行 tccutil reset ScreenCapture com.lumora.app），然后重新启动 Lumora。";
    ResetPermissionClose => "Close", "关闭";

    // -- install location (v0.6.1) ----------------------------------------------
    LocationTitle => "Move Lumora to the Applications folder?", "要将 Lumora 移到“应用程序”文件夹吗？";
    LocationTranslocated => "macOS is running Lumora from a temporary, read-only copy, so the Screen Recording permission may not stick and Lumora may not appear in System Settings.\n\nMove it to Applications and Lumora reopens from there.",
        "macOS 正从一个临时的只读副本运行 Lumora，因此“屏幕录制”权限可能无法保留，Lumora 也可能不会出现在“系统设置”中。\n\n移到“应用程序”后，Lumora 会从那里重新打开。";
    LocationDiskImage => "Lumora is running from the disk image. Permissions granted now won’t carry over to the copy you install, and the disk image goes away when ejected.\n\nMove it to Applications and Lumora reopens from there.",
        "Lumora 正从磁盘映像中运行。现在授予的权限不会转移到你安装的副本，推出磁盘映像后它也会消失。\n\n移到“应用程序”后，Lumora 会从那里重新打开。";
    MoveToApplications => "Move to Applications", "移到“应用程序”";
    LocationMoveFailed => "Couldn’t move Lumora to Applications", "无法将 Lumora 移到“应用程序”";
    LocationMoveFailedDetail => "{0}\n\nDrag Lumora into the Applications folder in Finder yourself, then open it from there.",
        "{0}\n\n请在“访达”中手动将 Lumora 拖到“应用程序”文件夹，然后从那里打开。";

    // -- settings -----------------------------------------------------------------
    SettingsTitle => "Lumora Settings", "Lumora 设置";
    TabGeneral => "General", "通用";
    TabShortcuts => "Shortcuts", "快捷键";
    TabCapture => "Capture", "截图";
    TabRecording => "Recording", "录制";
    TabAi => "AI Writing", "AI 写作";
    SaveCapturesTo => "Save captures to:", "截图保存到：";
    Choose => "Choose…", "选择…";
    ChoosePrompt => "Choose", "选择";
    FileName => "File name:", "文件名：";
    FileNameTokens => "Tokens: {date} {time} {YYYY} {MM} {DD} {hh} {mm} {ss} {kind}",
        "可用标记：{date} {time} {YYYY} {MM} {DD} {hh} {mm} {ss} {kind}";
    AfterCapture => "After capture:", "截图后：";
    CopyToClipboard => "Copy to clipboard", "拷贝到剪贴板";
    SaveToDisk => "Save to disk", "保存到磁盘";
    SaveEditableDocument => "Also save an editable .lumora document", "同时保存可编辑的 .lumora 文档";
    ShowQuickAccess => "Show Quick Access card", "显示快速访问卡片";
    OpenEditorAfterCapture => "Open annotation editor", "打开标注编辑器";
    HideCardAfter => "Hide card after:", "卡片隐藏时间：";
    Seconds3 => "3 seconds", "3 秒";
    Seconds5 => "5 seconds", "5 秒";
    Seconds6 => "6 seconds", "6 秒";
    Seconds10 => "10 seconds", "10 秒";
    Seconds30 => "30 seconds", "30 秒";
    PlaySounds => "Play sounds", "播放声音";
    LaunchAtLogin => "Launch at login", "登录时启动";
    LanguageLabel => "Language:", "语言：";
    LanguageSystem => "System Default", "跟随系统";
    LanguageNote => "Menus and windows update immediately.", "菜单和窗口会立即更新。";
    TypeShortcut => "Type shortcut…", "请按快捷键…";
    ClickToRecord => "Click to record", "点击录制";
    AddModifier => "Add ⌃, ⌥ or ⌘", "请加上 ⌃、⌥ 或 ⌘";
    RestoreDefaults => "Restore Defaults", "恢复默认";
    ShortcutsNote => "Click a field and press keys. ⌫ clears, Esc cancels.", "点击输入框后按下按键。⌫ 清除，Esc 取消。";
    ConflictDuplicate => "“{0}” and “{1}” use the same shortcut.", "“{0}”和“{1}”使用了相同的快捷键。";
    ConflictSystem => "“{0}” conflicts with a macOS screenshot shortcut.", "“{0}”与 macOS 截图快捷键冲突。";
    ConflictTaken => "{0} is already used by another app — choose a different shortcut for “{1}”.",
        "{0} 已被其他应用占用——请为“{1}”换一个快捷键。";
    AreaSelection => "Area selection:", "区域选择：";
    ShowMagnifier => "Show magnifier", "显示放大镜";
    ShowCrosshair => "Show crosshair guides", "显示十字参考线";
    HighlightWindows => "Highlight windows on hover", "悬停时高亮窗口";
    WindowCaptureLabel => "Window capture:", "窗口截图：";
    IncludeShadow => "Include window shadow", "包含窗口阴影";
    ResolutionLabel => "Resolution:", "分辨率：";
    SaveRetina => "Save at Retina (native) scale", "以 Retina（原生）分辨率保存";
    CaptureNote => "Cropped from the frozen screen at native resolution.",
        "区域截图从冻结的屏幕画面中按原生分辨率裁剪。";
    TextRecognitionLabel => "Capture Text:", "识别文字：";
    TextActionsPanel => "Show actions panel after Capture Text", "识别文字后显示操作面板";
    TextRecognitionNote => "English, Chinese (Simp. & Trad.) and Japanese.",
        "支持识别英文、简体中文、繁体中文和日文。";
    FrameRate => "Frame rate:", "帧率：";
    Fps30 => "30 fps", "30 帧/秒";
    Fps60 => "60 fps", "60 帧/秒";
    VideoFormat => "Video format:", "视频格式：";
    H264 => "H.264 (most compatible)", "H.264（兼容性最好）";
    Hevc => "HEVC (smaller files)", "HEVC（文件更小）";
    Countdown => "Countdown:", "倒计时：";
    Off => "Off", "关闭";
    Include => "Include:", "包含：";
    MousePointer => "Mouse pointer", "鼠标指针";
    SystemAudio => "System audio", "系统声音";
    Microphone => "Microphone", "麦克风";
    MicrophoneNote => "Lumora asks for microphone access on first use.", "首次使用麦克风录制时，Lumora 会请求麦克风权限。";
    MicrophoneDevice => "Microphone device:", "麦克风设备：";
    MicrophoneDefault => "System default", "系统默认设备";
    ShowClicks => "Click ripples", "点击涟漪效果";
    KeystrokeHud => "Keystrokes (shortcuts and special keys)", "按键提示（快捷键与特殊键）";
    KeystrokePosition => "Keystroke position:", "按键位置：";
    PositionBottomCenter => "Bottom center", "底部居中";
    PositionBottomLeft => "Bottom left", "左下角";
    PositionBottomRight => "Bottom right", "右下角";
    PositionTopCenter => "Top center", "顶部居中";
    KeystrokeHudNote => "Shows shortcuts like ⌘⇧4 and keys like ↩ or ⌫ in the video; plain typing is never shown. Needs Input Monitoring permission.", "在视频中显示 ⌘⇧4 等快捷键和 ↩、⌫ 等按键，普通输入的文字永不显示。需要“输入监控”权限。";
    ToastKeystrokesUnavailable => "Recording without keystrokes", "本次录制不显示按键";
    KeystrokesAccessDetail => "Allow Lumora in System Settings → Privacy & Security → Input Monitoring, then start a new recording.", "请在“系统设置 → 隐私与安全性 → 输入监控”中允许 Lumora，然后重新开始录制。";
    Visualize => "Show:", "显示：";
    ToastMicUnavailable => "Recording without microphone", "本次录制不含麦克风声音";
    ErrMicPermission => "Allow Lumora in System Settings › Privacy & Security › Microphone.", "请在“系统设置 › 隐私与安全性 › 麦克风”中允许 Lumora。";

    // -- annotation editor --------------------------------------------------------
    EditorTitle => "Annotate — {0}", "标注 — {0}";
    ToolSelect => "Select (V)", "选择（V）";
    ToolArrow => "Arrow (A)", "箭头（A）";
    ToolRect => "Rectangle (R)", "矩形（R）";
    ToolEllipse => "Ellipse (O)", "椭圆（O）";
    ToolLine => "Line (L)", "直线（L）";
    ToolPen => "Pen (P)", "画笔（P）";
    ToolHighlighter => "Highlighter (H)", "荧光笔（H）";
    ToolText => "Text (T)", "文字（T）";
    ToolStep => "Numbered Step (N)", "序号（N）";
    ToolRedact => "Pixelate / Blur (B)", "马赛克 / 模糊（B）";
    ToolSpotlight => "Spotlight (S)", "聚光灯（S）";
    ToolCrop => "Crop (C)", "裁剪（C）";
    OptionCurved => "Curved", "弯曲";
    OptionRounded => "Rounded", "圆角";
    OptionBackground => "Background", "背景";
    OptionPixelate => "Pixelate", "马赛克";
    OptionBlur => "Blur", "模糊";
    CustomColor => "Custom Colour…", "自定义颜色…";
    StrokeThin => "Thin (1)", "细（1）";
    StrokeMedium => "Medium (2)", "中（2）";
    StrokeThick => "Thick (3)", "粗（3）";
    ToolbarOpacity => "Opacity", "不透明度";
    OpacityTip => "Opacity: {0}%", "不透明度：{0}%";
    ShapeStyle => "Shape Style", "形状样式";
    SectionFill => "Fill", "填充";
    FillNone => "No Fill", "无填充";
    FillSolid => "Solid", "实心";
    FillTranslucent => "Translucent", "半透明";
    SectionHeads => "Arrowheads", "箭头样式";
    HeadsNone => "None", "无箭头";
    HeadsFilled => "Filled", "实心箭头";
    HeadsOpen => "Open", "开口箭头";
    HeadsBoth => "Both Ends", "双向箭头";
    SectionLine => "Line", "线条";
    DashSolid => "Solid Line", "实线";
    DashDashed => "Dashed", "虚线";
    DashDotted => "Dotted", "点线";
    Undo => "Undo (⌘Z)", "撤销（⌘Z）";
    Redo => "Redo (⇧⌘Z)", "重做（⇧⌘Z）";
    CopyImage => "Copy (⌘C)", "拷贝（⌘C）";
    SaveImage => "Save (⌘S)", "保存（⌘S）";
    DragImage => "Drag to share", "拖出以分享";
    ApplyCrop => "Apply Crop (↩)", "应用裁剪（↩）";
    ResetCrop => "Reset Crop", "重置裁剪";

    // -- toasts -------------------------------------------------------------------
    ToastCopied => "Copied to clipboard", "已拷贝到剪贴板";
    ToastSaved => "Saved", "已保存";
    ToastTextCopied => "Text copied", "文字已拷贝";
    ToastNoText => "No text found", "未识别到文字";
    ToastNoTextDetail => "Try a larger area or higher-contrast text.", "请尝试更大的区域或对比度更高的文字。";
    ToastCaptureFailed => "Capture failed", "截图失败";
    ToastSaveFailed => "Couldn’t save the file", "无法保存文件";
    ToastRecordingFailed => "Recording failed", "录制失败";
    ToastShortcutTaken => "Shortcut unavailable", "快捷键不可用";
    ToastShortcutTakenDetail => "{0} is used by another app. Choose another in Settings.", "{0} 已被其他应用占用，请在设置中更换。";
    ToastTextFailed => "Text recognition failed", "文字识别失败";

    // -- beautify -----------------------------------------------------------------
    Beautify => "Beautify", "美化";
    BeautifyBackground => "Background", "背景";
    BeautifyPadding => "Padding", "内边距";
    BeautifyCornerRadius => "Corner radius", "圆角";
    BeautifyShadow => "Shadow", "阴影";
    BeautifyShadowBlur => "Shadow blur", "阴影模糊";
    BeautifyShadowOpacity => "Shadow opacity", "阴影浓度";
    BeautifyAutoBalance => "Auto-balance", "自动平衡";
    BeautifyAspectRatio => "Aspect ratio", "宽高比";
    BeautifyAspectAuto => "Auto", "自动";
    BeautifyWindowChrome => "Window frame", "窗口外框";
    BeautifyChromeLight => "Light", "浅色";
    BeautifyChromeDark => "Dark", "深色";
    BeautifyNone => "None", "无";
    BeautifySolid => "Solid colour", "纯色";
    BeautifyGradient => "Gradient", "渐变";
    BeautifyImage => "Image…", "图片…";
    PresetSunset => "Sunset", "日落";
    PresetOcean => "Ocean", "海洋";
    PresetAurora => "Aurora", "极光";
    PresetPeach => "Peach", "蜜桃";
    PresetMint => "Mint", "薄荷";
    PresetGraphite => "Graphite", "石墨";
    PresetLavender => "Lavender", "薰衣草";
    PresetMidnight => "Midnight", "午夜";

    // -- errors -------------------------------------------------------------------
    ErrPermission => "Lumora needs Screen Recording permission.", "Lumora 需要“屏幕录制”权限。";
    ErrTargetGone => "The {0} to capture is no longer available.", "要截取的{0}已不可用。";
    ErrNothing => "Nothing to capture.", "没有可截取的内容。";
    ErrWrite => "Could not write {0}.", "无法写入 {0}。";
    ErrOpenDocument => "Couldn’t open “{0}”", "无法打开“{0}”";
    ErrDocumentDetail => "This Lumora document can’t be read: {0}", "无法读取此 Lumora 文档：{0}";
    ErrDocumentImage => "the embedded image couldn’t be decoded", "无法解码内嵌的图像";
    TargetDisplay => "display", "显示器";
    TargetWindow => "window", "窗口";

    // -- AI writing (v0.4) ----------------------------------------------------------
    AiMenu => "AI Writing", "AI 写作";
    AiProviderQwen => "Qwen (Alibaba Cloud)", "通义千问（阿里云）";
    AiProviderMoonshot => "Moonshot (Kimi)", "月之暗面（Kimi）";
    AiProviderOllama => "Ollama (local)", "Ollama（本地）";
    AiProviderCustom => "Custom (OpenAI-compatible)", "自定义（OpenAI 兼容）";
    AiOutputAuto => "Match the screenshot", "跟随截图语言";
    AiTaskCaption => "Write caption", "生成说明";
    AiTaskStepGuide => "Step guide", "生成步骤说明";
    AiTaskAltText => "Alt text", "生成替代文本";
    AiTaskPolish => "Polish", "润色";
    AiTaskTranslate => "Translate", "翻译";
    AiTaskSummarize => "Summarize", "总结";
    AiTaskExtractTable => "Extract table", "提取表格";
    AiTaskExtractCode => "Extract code", "提取代码";
    AiTaskChecklist => "Checklist", "待办清单";
    TextAsTable => "As Table", "转为表格";
    TextAsCode => "As Code", "转为代码";
    TextResultTitle => "Recognised Text", "识别的文字";
    TextCopyCsv => "Copy as CSV", "拷贝为 CSV";
    TextCopy => "Copy Text", "拷贝文字";
    TextAiOffHint => "Turn on AI writing in Settings → AI to use this.", "请在“设置 → AI”中开启 AI 写作后使用。";
    TextLocalResult => "On-device result", "本地识别结果";
    TextRefining => "Refining with AI…", "正在用 AI 优化…";
    TextAiKeptLocal => "AI failed; showing the on-device result. {0}", "AI 处理失败，显示本地结果。{0}";
    ToolbarMore => "More tools", "更多工具";
    ToolbarColor => "Color", "颜色";
    ToolbarStroke => "Stroke", "线宽";
    AiErrMissingKey => "Add an API key in Settings → AI.", "请在“设置 → AI”中添加 API 密钥。";
    AiErrBadConfig => "Set a base URL and model in Settings → AI.", "请在“设置 → AI”中填写接口地址和模型。";
    AiErrEmptyInput => "There’s no text to work with. Add text or allow sending the image.", "没有可处理的文字。请添加文字，或允许发送图片。";
    AiErrBadKey => "The API key was rejected. Check it in Settings → AI.", "API 密钥无效，请在“设置 → AI”中检查。";
    AiErrRateLimited => "Rate limit or quota reached. Try again later.", "已达到速率限制或额度上限，请稍后再试。";
    AiErrNotFound => "Model or endpoint not found. Check the base URL and model name.", "未找到模型或接口，请检查接口地址和模型名称。";
    AiErrServer => "The AI service had a problem (HTTP {0}). Try again later.", "AI 服务出现问题（HTTP {0}），请稍后再试。";
    AiErrHttp => "The AI service returned HTTP {0}: {1}", "AI 服务返回 HTTP {0}：{1}";
    AiErrTimeout => "The AI service took too long to respond.", "AI 服务响应超时。";
    AiErrNetwork => "Couldn’t reach the AI service: {0}", "无法连接 AI 服务：{0}";
    AiErrBadResponse => "Unexpected response from the AI service: {0}", "AI 服务返回了无法识别的内容：{0}";
    AiErrCancelled => "Cancelled", "已取消";
    AiErrKeychain => "Couldn’t access the Keychain: {0}", "无法访问钥匙串：{0}";
    AiEnableLabel => "AI writing:", "AI 写作：";
    AiEnable => "Enable AI writing in the editor", "在编辑器中启用 AI 写作";
    AiProviderLabel => "Provider:", "服务商：";
    AiBaseUrl => "Base URL:", "接口地址：";
    AiModel => "Model:", "模型：";
    AiApiKey => "API key:", "API 密钥：";
    AiKeyPlaceholder => "Stored in the macOS Keychain", "保存在 macOS 钥匙串中";
    AiNoKeyNeeded => "No key needed — leave it blank.", "无需密钥，留空即可。";
    AiKeyOptional => "Optional — only if your endpoint requires one.", "可选，仅在接口需要时填写。";
    AiTemperature => "Temperature:", "温度：";
    AiOutputLanguage => "Output language:", "输出语言：";
    AiImageLabel => "Screenshot:", "截图：";
    AiSendImage => "Send screenshot image", "发送截图图片";
    AiVisionHint => "This model may not accept images.", "此模型可能不支持图片。";
    AiTutorialLabel => "Tutorials:", "教程：";
    AiTutorialSteps => "Write steps with AI (text only)", "用 AI 撰写教程步骤（仅发送文字）";
    // -- tutorials ----------------------------------------------------------------
    MakeTutorialMenu => "Make Tutorial from Recording…", "从录屏生成教程…";
    MakeTutorial => "Make Tutorial", "生成教程";
    TutorialVerbClick => "Click", "点击";
    TutorialVerbDoubleClick => "Double-click", "双击";
    TutorialVerbRightClick => "Right-click", "右键点击";
    TutorialStepOn => "{0} “{1}”", "{0}“{1}”";
    TutorialStepIn => "{0} in {1}", "在 {1} 中{0}";
    TutorialStepHere => "{0} here", "在此处{0}";
    TutorialInApp => "In {0}.", "位于 {0}。";
    TutorialInAppWindow => "In {0} — {1}.", "位于 {0} — {1}。";
    TutorialStepN => "Step {0}", "第 {0} 步";
    TutorialHeading => "Step {0}: {1}", "第 {0} 步：{1}";
    TutorialFooter => "Made with Lumora", "由 Lumora 生成";
    TutorialWindowTitle => "Tutorial", "教程";
    TutorialExportMarkdown => "Export Markdown…", "导出 Markdown…";
    TutorialExportHtml => "Export HTML…", "导出 HTML…";
    TutorialCopyMarkdown => "Copy Markdown", "拷贝 Markdown";
    TutorialDeleteStep => "Delete Step", "删除步骤";
    TutorialTitlePlaceholder => "Step title", "步骤标题";
    TutorialDetailPlaceholder => "Details (optional)", "说明（可选）";
    TutorialSummary => "{0} steps", "{0} 个步骤";
    TutorialSummaryAi => "{0} steps · written by AI", "{0} 个步骤 · 由 AI 撰写";
    ToastTutorialWorking => "Making tutorial…", "正在生成教程…";
    ToastTutorialWriting => "Writing steps with AI…", "正在用 AI 撰写步骤…";
    ToastTutorialReady => "Tutorial ready", "教程已生成";
    ToastTutorialCapped => "Kept the first {0} steps", "仅保留前 {0} 个步骤";
    ToastTutorialFailed => "Couldn’t make tutorial", "无法生成教程";
    ToastTutorialNoClicks => "No clicks inside the recording", "录制区域内没有点击";
    ToastTutorialNoClicksDetail => "Tutorials are made from the clicks Lumora logged while recording.", "教程根据录制时记录的点击生成。";
    ToastTutorialNoLog => "This recording has no click log", "此录屏没有点击记录";
    ToastTutorialNoLogDetail => "Only recordings made with Lumora 0.5 or later have one ({0}).", "只有 Lumora 0.5 及以上版本录制的视频才有（{0}）。";
    ToastTutorialAiFallback => "AI write-up failed; using basic titles", "AI 撰写失败，已使用基础标题";
    ToastTutorialExported => "Tutorial exported", "教程已导出";
    ToastTutorialCopied => "Markdown copied", "已拷贝 Markdown";

    // -- subtitles ----------------------------------------------------------------
    TranscribeMenu => "Transcribe Recording…", "转写录屏…";
    CancelTranscribeMenu => "Cancel Transcription", "取消转写";
    MakeSubtitles => "Subtitles", "字幕";
    TranscribeEngineLabel => "Speech recognition:", "语音识别：";
    TranscribeOnDevice => "On this Mac", "本机识别";
    TranscribeCloud => "Cloud (AI provider)", "云端（AI 服务商）";
    TranscribeLanguageLabel => "Spoken language:", "语音语言：";
    TranscribeLanguageAuto => "Same as app", "跟随应用语言";
    TranscribeLanguageZh => "Chinese (Mandarin)", "中文（普通话）";
    TranscribeLanguageEn => "English", "英语";
    TranscribeAudioLabel => "Audio source:", "音频来源：";
    TranscribeAudioMic => "Microphone", "麦克风";
    TranscribeAudioSystem => "System audio", "系统声音";
    TranscribeAudioMixed => "Both (mixed)", "两者混合";
    TranscribeCloudModel => "Cloud model:", "云端模型：";
    TranscribeStrict => "Never send audio to Apple servers", "绝不将音频发送到 Apple 服务器";
    TranscriptWindowTitle => "Subtitles", "字幕";
    TranscriptExportSrt => "Export SRT…", "导出 SRT…";
    TranscriptExportVtt => "Export VTT…", "导出 VTT…";
    TranscriptCopyChapters => "Copy Chapters", "拷贝章节";
    TranscriptCopySummary => "Copy Summary", "拷贝摘要";
    TranscriptSummaryHeading => "Summary", "摘要";
    TranscriptChaptersHeading => "Chapters", "章节";
    TranscriptSegmentsHeading => "Transcript", "转写文本";
    TranscriptSummaryPlaceholder => "No summary", "暂无摘要";
    TranscriptInfo => "{0} segments", "{0} 段";
    ToastTranscribeExporting => "Preparing audio…", "正在准备音频…";
    ToastTranscribeExportingDetail => "To cancel, choose Cancel Transcription in the menu bar.", "如需取消，请在菜单栏中选择“取消转写”。";
    ToastTranscribeWorking => "Transcribing…", "正在转写…";
    ToastTranscribeSummarizing => "Writing summary and chapters with AI…", "正在用 AI 撰写摘要和章节…";
    ToastTranscribeReady => "Subtitles ready", "字幕已生成";
    ToastTranscribeFailed => "Couldn’t transcribe the recording", "无法转写录屏";
    ToastSpeechOnDeviceUnavailable => "On-device speech recognition isn’t available", "本机离线语音识别不可用";
    ToastTranscribeCancelled => "Transcription cancelled", "已取消转写";
    ToastTranscribeNoSpeech => "No speech found in the recording", "录屏中没有识别到语音";
    ToastTranscribeTooLarge => "The audio is too large to upload ({0} MB; the limit is 25 MB)", "音频过大，无法上传（{0} MB，上限为 25 MB）";
    ToastTranscribeTooLargeDetail => "Switch Speech recognition to “On this Mac” in Settings → Recording.", "请在“设置 → 录制”中将语音识别切换为“本机识别”。";
    ToastTranscribeCloudOff => "Turn on AI in Settings → AI Writing to use cloud recognition", "请先在“设置 → AI 写作”中开启 AI 才能使用云端识别";
    ToastTranscribeAiFallback => "AI summary failed; using basic chapters", "AI 摘要失败，已使用基础章节";
    ToastTranscribeExported => "Subtitles exported", "字幕已导出";
    ToastChaptersCopied => "Chapters copied", "已拷贝章节";
    ToastSummaryCopied => "Summary copied", "已拷贝摘要";
    ErrSpeechPermission => "Lumora needs Speech Recognition permission (System Settings → Privacy & Security → Speech Recognition).", "Lumora 需要语音识别权限（系统设置 → 隐私与安全性 → 语音识别）。";
    ErrSpeechUnavailable => "Speech recognition is unavailable: {0}", "语音识别不可用：{0}";
    ErrSpeechUnavailableStrict => "This Mac can’t recognize {0} without Apple’s servers. To transcribe, switch Speech recognition to “Cloud (AI provider)” in Settings → Recording, or turn off “Never send audio to Apple servers”.", "本机无法在不使用 Apple 服务器的情况下识别{0}。如需转写，请在“设置 → 录制”中将语音识别切换为“云端（AI 服务商）”，或关闭“绝不将音频发送到 Apple 服务器”。";
    ErrSpeechLocaleUnsupported => "Apple Speech doesn’t support {0} on this Mac. Switch Speech recognition to “Cloud (AI provider)” in Settings → Recording.", "此 Mac 上的 Apple 语音识别不支持{0}。请在“设置 → 录制”中将语音识别切换为“云端（AI 服务商）”。";
    ErrSpeechNotReady => "Apple Speech isn’t available right now. Check that Siri & Dictation is allowed in System Settings, or switch Speech recognition to “Cloud (AI provider)” in Settings → Recording.", "Apple 语音识别暂时不可用。请检查系统设置中是否允许 Siri 与听写，或在“设置 → 录制”中将语音识别切换为“云端（AI 服务商）”。";
    ErrReadAudio => "Couldn’t read the recording’s audio. Try again, or check that the recording still exists.", "无法读取录屏的音频。请重试，或确认录屏文件仍然存在。";
    ErrSpeech => "Speech recognition failed: {0}", "语音识别失败：{0}";
    ErrNoAudio => "The recording has no audio.", "此录屏没有音频。";
    ErrCancelled => "Cancelled.", "已取消。";
    AiTimeout => "Timeout:", "超时：";
    AiSecondsUnit => "seconds (5–600)", "秒（5–600）";
    AiTestConnection => "Test Connection", "测试连接";
    AiTesting => "Testing…", "正在测试…";
    AiTestOk => "Connected.", "连接成功。";
    AiTestModelMissing => "Connected, but the model wasn’t found in the list.", "已连接，但列表中未找到该模型。";
    AiPrivacyLabel => "Privacy:", "隐私：";
    AiPrivacyNote => "By default only recognised text and annotation text are sent. The screenshot image is sent only if you turn it on above. Your API key stays in the macOS Keychain.",
        "默认只发送识别出的文字和标注文字；只有在上方开启后才会发送截图图片。API 密钥只保存在 macOS 钥匙串中。";
    AiMenuTip => "AI Writing (⌘J)", "AI 写作（⌘J）";
    AiMenuDisabledTip => "Enable in Settings → AI Writing", "请在“设置 → AI”中启用 AI 写作";
    AiOpenSettings => "Open AI Settings", "打开 AI 设置";
    AiStop => "Stop", "停止";
    AiRetry => "Retry", "重试";
    AiInsert => "Insert as Text", "插入为文字";
    AiReplace => "Replace Text", "替换文字";
    AiClose => "Close", "关闭";
    AiReadingText => "Reading the screenshot’s text…", "正在识别截图中的文字…";
    AiWriting => "Writing…", "正在生成…";
    AiCopied => "Copied to the clipboard", "已拷贝到剪贴板";
    AiStopped => "Stopped.", "已停止。";
    ToolSmartRedact => "Smart Redact (⌘⇧R)", "智能打码（⌘⇧R）";
    ToolSmartRedactBusy => "Finding sensitive info…", "正在查找敏感信息…";
    ToastRedacted => "Redacted {0} items", "已打码 {0} 处";
    ToastRedactedOne => "Redacted 1 item", "已打码 1 处";
    ToastRedactUndo => "Press ⌘Z to undo", "按 ⌘Z 撤销";
    ToastRedactNothing => "Nothing sensitive found", "未发现敏感信息";
    ToastRedactNothingDetail => "Use the Pixelate / Blur tool to hide anything by hand.", "可用马赛克 / 模糊工具手动遮盖。";
    ToastRedactFailed => "Smart redaction failed", "智能打码失败";
    RedactAiFailed => "AI check failed; local results only.", "AI 检查失败，仅使用本地检测结果。";
    ToastAutoRedacted => "Auto-redacted {0} items", "已自动打码 {0} 处";
    RedactSectionLabel => "Smart redaction:", "智能打码：";
    RedactEmail => "Email addresses", "邮箱地址";
    RedactPhone => "Phone numbers", "电话号码";
    RedactIdCard => "ID card numbers", "身份证号";
    RedactBankCard => "Bank card numbers", "银行卡号";
    RedactSecret => "API keys & tokens", "API 密钥与令牌";
    RedactIp => "IP addresses", "IP 地址";
    RedactUrlToken => "Tokens in links", "链接中的令牌";
    RedactFaces => "Faces", "人脸";
    RedactStyleLabel => "Redaction style:", "打码样式：";
    RedactAiCheck => "Also check with AI (sends text only)", "同时用 AI 检查（仅发送文字）";
    RedactAuto => "Redact new screenshots automatically", "复制 / 保存新截图前自动打码";
    RedactNote => "Detects on this Mac; AI check needs AI Writing on.", "检测在本机完成；AI 检查需先启用 AI 写作。";

    // -- capture history (v0.6) ---------------------------------------------------
    CaptureHistory => "Capture History", "截图历史";
    HistoryFilterAll => "All", "全部";
    HistoryFilterScreenshots => "Screenshots", "截图";
    HistoryFilterRecordings => "Recordings", "录屏";
    HistoryFilterDocuments => "Editable", "可编辑";
    HistorySearchPlaceholder => "Search names and text", "搜索文件名和文字";
    HistoryCount => "{0} items", "共 {0} 项";
    HistoryEmptyTitle => "No captures yet", "还没有截图或录屏";
    HistoryEmptyDetail => "Screenshots and recordings you save appear here.", "保存的截图和录屏会显示在这里。";
    HistoryNoMatches => "Nothing matches your search", "没有符合搜索条件的项目";
    HistoryLoading => "Loading history", "正在载入历史记录";
    HistoryReopen => "Open in Editor", "在编辑器中打开";
    HistoryPlay => "Play", "播放";
    HistoryShowInFinder => "Show in Finder", "在访达中显示";
    HistoryMoveToTrash => "Move to Trash", "移到废纸篓";
    HistoryShowMore => "Show More", "显示更多";
    HistoryDeleteTitle => "Move “{0}” to the Trash?", "将“{0}”移到废纸篓？";
    HistoryDeleteDetail => "Its editable document and recording sidecars go with it. You can put them back from the Trash.",
        "它的可编辑文档和录屏附属文件也会一起移走，可以从废纸篓中放回。";
    HistoryRetentionBanner => "{0} captures are older than {1} days.", "有 {0} 项已超过 {1} 天。";
    HistoryRetentionButton => "Move to Trash…", "移到废纸篓…";
    HistoryRetentionConfirmTitle => "Move {0} captures older than {1} days to the Trash?", "将超过 {1} 天的 {0} 项移到废纸篓？";
    HistoryRetentionConfirmDetail => "Their editable documents and recording sidecars go too. You can put them back from the Trash.",
        "它们的可编辑文档和录屏附属文件也会一起移走，可以从废纸篓中放回。";
    HistoryKeepAll => "Keep everything", "全部保留";
    HistoryKeep30 => "30 days", "30 天";
    HistoryKeep90 => "90 days", "90 天";
    HistoryRetentionLabel => "Keep history:", "历史保留：";
    HistoryRetentionNote => "Older captures are only offered for the Trash in the history window, never deleted on their own.",
        "超期的项目只会在历史窗口中提示移到废纸篓，不会被自动删除。";
    HistoryTrashFailed => "Couldn’t move to the Trash", "无法移到废纸篓";
    HistoryTrashFailedDetail => "{0} could not be moved. Nothing was deleted.", "无法移动 {0}，没有删除任何文件。";
    HistoryMissingFile => "That file is no longer there", "该文件已不存在";
    HistoryCancel => "Cancel", "取消";

    // -- trim & GIF ---------------------------------------------------------------
    CardTrim => "Trim / GIF…", "剪辑 / GIF…";
    HistoryTrim => "Trim or Export GIF…", "剪辑或导出 GIF…";
    TrimWindowTitle => "Trim Recording", "剪辑录屏";
    TrimStart => "Start:", "开始：";
    TrimEnd => "End:", "结束：";
    TrimLength => "Length: {0} of {1}", "时长：{0}（共 {1}）";
    TrimSave => "Save Trimmed", "保存剪辑";
    GifWidthLabel => "GIF width:", "GIF 宽度：";
    GifWidthOriginal => "Original", "原始尺寸";
    GifFpsLabel => "Frame rate:", "帧率：";
    GifFps => "{0} fps", "{0} 帧/秒";
    GifEstimate => "About {0} · {1} frames · {2}", "约 {0} · {1} 帧 · {2}";
    GifCapped => "only the first {0} frames", "仅前 {0} 帧";
    GifExport => "Export GIF…", "导出 GIF…";
    ClipTrimming => "Trimming…", "正在剪辑…";
    ClipMakingGif => "Making GIF…", "正在生成 GIF…";
    ToastTrimDone => "Recording trimmed", "录屏已剪辑";
    ToastTrimStepsDropped => "{0} — tutorial steps weren’t kept; make them again from the trimmed recording.", "{0} — 教程步骤未保留，请用剪辑后的录屏重新生成。";
    ToastTrimFailed => "Couldn’t trim the recording", "无法剪辑录屏";
    ToastTrimNothing => "Move Start or End to choose a shorter part first", "请先拖动开始或结束，选择更短的片段";
    ToastGifSaved => "GIF saved", "GIF 已保存";
    ToastGifFailed => "Couldn’t make the GIF", "无法生成 GIF";
    ToastClipCancelled => "Export cancelled", "已取消导出";
    ToastClipBusy => "Wait for the current export to finish", "请等待当前导出完成";
    TranscriptBurnIn => "Burn In Subtitles…", "烧录字幕…";
    SubtitleSizeLabel => "Size:", "字号：";
    SubtitleSizeSmall => "Small", "小";
    SubtitleSizeMedium => "Medium", "中";
    SubtitleSizeLarge => "Large", "大";
    SubtitlePositionLabel => "Position:", "位置：";
    SubtitlePositionBottom => "Bottom", "底部";
    SubtitlePositionTop => "Top", "顶部";
    SubtitleBackgroundBox => "Background box", "背景框";
    ClipBurningSubtitles => "Burning in subtitles…", "正在烧录字幕…";
    ToastSubtitlesBurned => "Subtitled recording saved", "已保存带字幕的录屏";
    ToastSubtitlesBurnFailed => "Couldn’t burn in the subtitles", "无法烧录字幕";
    ToastSubtitlesNone => "There are no subtitles to burn in", "没有可烧录的字幕";
    SubtitlePreviewLoading => "Loading preview…", "正在载入预览…";
    SubtitlePreviewNoVideo => "This recording has no video, so there is no subtitle preview.", "此录音没有画面，无法预览字幕。";
    SubtitlePreviewFailed => "Couldn’t load a preview frame", "无法载入预览画面";

    // -- scrolling capture (v0.6) -------------------------------------------------
    ScrollingCapture => "Scrolling Capture", "滚动截图";
    HintScroll => "Drag to choose the area to scroll through  ·  Esc to cancel", "拖动选择要滚动截取的区域  ·  按 Esc 取消";
    ScrollPrompt => "Scroll slowly…", "请缓慢滚动…";
    ScrollFrames => "{0} frames", "已截取 {0} 帧";
    ScrollDone => "Done", "完成";
    ScrollCancel => "Cancel", "取消";
    ToastScrollCapped => "Scrolling capture reached its height limit", "滚动截图已达到高度上限";
    ToastScrollNoOverlap => "Lost track of the scrolling, so the capture stopped there", "无法衔接滚动内容，截图已在此处停止";
    ToastScrollNoOverlapDetail => "Scroll a little slower next time.", "下次请滚动得慢一些。";
    ToastScrollStopped => "Scrolling capture stopped: the screen could not be captured", "滚动截图已停止：无法截取屏幕";
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(s: &str) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'{' {
                if let Some(end) = s[i..].find('}') {
                    v.push(s[i..i + end + 1].to_string());
                    i += end;
                }
            }
            i += 1;
        }
        v.sort();
        v
    }

    #[test]
    fn every_key_has_both_languages() {
        for &m in Msg::ALL {
            for lang in Language::ALL {
                let s = m.text(lang);
                assert!(!s.trim().is_empty(), "{m:?} is empty in {lang:?}");
                assert_eq!(s, s.trim(), "{m:?} has stray whitespace in {lang:?}");
            }
        }
        assert!(Msg::ALL.len() > 100);
    }

    #[test]
    fn placeholders_match_across_languages() {
        for &m in Msg::ALL {
            assert_eq!(placeholders(m.text(Language::En)), placeholders(m.text(Language::ZhHans)), "{m:?}");
        }
    }

    #[test]
    fn ellipses_and_shortcut_glyphs_are_consistent() {
        for &m in Msg::ALL {
            let (en, zh) = (m.text(Language::En), m.text(Language::ZhHans));
            assert_eq!(en.ends_with('…'), zh.ends_with('…'), "{m:?} ellipsis");
            for glyph in ['⌘', '⇧', '⌥', '⌃', '⌫', '↩'] {
                assert_eq!(en.contains(glyph), zh.contains(glyph), "{m:?} glyph {glyph}");
            }
        }
    }

    #[test]
    fn chinese_is_actually_translated() {
        // Everything except proper nouns, numbers and codec names differs.
        let same: Vec<Msg> =
            Msg::ALL.iter().copied().filter(|m| m.text(Language::En) == m.text(Language::ZhHans)).collect();
        assert_eq!(same, vec![Msg::AppName]);
    }

    #[test]
    fn language_resolution() {
        assert_eq!(Language::from_tag("zh-Hans-CN"), Some(Language::ZhHans));
        assert_eq!(Language::from_tag("zh_TW"), Some(Language::ZhHans));
        assert_eq!(Language::from_tag("en-GB"), Some(Language::En));
        assert_eq!(Language::from_tag("ja-JP"), None);
        assert_eq!(Language::from_preferred(&["ja-JP", "zh-Hans", "en"]), Language::ZhHans);
        assert_eq!(Language::from_preferred(&["fr-FR"]), Language::En);
        assert_eq!(Language::from_preferred::<&str>(&[]), Language::En);
        assert_eq!(LanguagePref::English.resolve(&["zh-Hans"]), Language::En);
        assert_eq!(LanguagePref::SimplifiedChinese.resolve(&["en"]), Language::ZhHans);
        assert_eq!(LanguagePref::System.resolve(&["zh-Hant-HK"]), Language::ZhHans);
    }

    #[test]
    fn pref_serde_uses_tags() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct W {
            l: LanguagePref,
        }
        let s = toml::to_string(&W { l: LanguagePref::SimplifiedChinese }).unwrap();
        assert_eq!(s.trim(), "l = \"zh-Hans\"");
        assert_eq!(toml::from_str::<W>("l = \"system\"").unwrap().l, LanguagePref::System);
        assert_eq!(LanguagePref::English.label(), "English");
    }

    #[test]
    fn formatting_replaces_numbered_placeholders() {
        let s = format_args_into("“{0}” and “{1}”", &[&"A", &2]);
        assert_eq!(s, "“A” and “2”");
        assert_eq!(Msg::CopiedHex.text(Language::ZhHans), "已复制 {0}");
    }

    #[test]
    fn global_language_switches() {
        // Single test touches the global to avoid cross-test races.
        set_language(Language::ZhHans);
        assert_eq!(language(), Language::ZhHans);
        assert_eq!(tr(Msg::Copy), "拷贝");
        assert_eq!(trf(Msg::CopiedHex, &[&"#FFFFFF"]), "已复制 #FFFFFF");
        set_language(Language::En);
        assert_eq!(tr(Msg::Copy), "Copy");
    }
}
