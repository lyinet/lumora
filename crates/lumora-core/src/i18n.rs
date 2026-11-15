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
    CaptureNote => "Area captures are cropped from the frozen screen at native resolution.",
        "区域截图从冻结的屏幕画面中按原生分辨率裁剪。";
    TextRecognitionLabel => "Capture Text:", "识别文字：";
    TextRecognitionNote => "Recognises English, Simplified & Traditional Chinese and Japanese.",
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
    MicrophoneNote => "Microphone capture requires macOS 15 or later.", "麦克风录制需要 macOS 15 或更高版本。";

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
