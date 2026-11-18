//! 微信宿主（WeChat host）在 Rust 侧的胶水。
//!
//! 适配层 `addons/bevy-wechat/runtime/adapter.js` 负责所有 `wx.*` 调用，并把
//! 宿主事实挂在 JS 全局对象上。游戏只读这些全局，不直接碰 `tt`：
//!
//! | 全局（默认前缀 `__bevy_wechat_`） | 方向 | 内容 |
//! | --- | --- | --- |
//! | `Version` | JS → Rust | 适配层版本，如 `"0.3.0"` |
//! | `Layout` | JS → Rust | `{canvas, safeArea, screen, devicePixelRatio, seq}` |
//! | `Canvas` / `SafeArea` | JS → Rust | 兼容 0.2 的扁平版本；0.3 仍会发布 |
//! | `Theme` / `ThemeManual` / `ThemeSource` | JS → Rust | 当前主题、是否手动覆盖、来源 |
//! | `SetTheme(value)` / `FollowTheme()` | Rust → JS | 覆盖主题 / 恢复跟随系统 |
//! | `RequestAd(kind)` | Rust → JS | 拉起激励视频，返回 ticket（0 表示失败/忙） |
//! | `AdTicket` / `AdResult` / `AdStatus` | JS → Rust | 票据与最终结果，用于关联回调 |
//! | `AppState` / `AppStateSeq` | JS → Rust | 前台/后台及变化序号 |
//! | `ViewportSeq` | JS → Rust | 画布/安全区变化序号 |
//!
//! 注意：微信的 wasm 运行时里 `eval` 不可用，`GameGlobal` 也不在 wasm 的
//! 全局对象上；适配层把 shim 挂在 `GameGlobal.__bevy_wechat_env`，因此这里
//! 先取 `env.window`，读不到再退回 `js_sys::global()` 上的 `window`
//! （开发者工具会预置一个只读的 `window`，两条路径必须区分）。

use bevy::prelude::*;

/// 画布的设计宽度（逻辑像素）。高度由画布宽高比推导。
pub const DESIGN_WIDTH: f32 = 720.0;
/// 适配层默认的全局前缀；必须和 `game.js` 的 `globalPrefix` 一致。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const GLOBAL_PREFIX: &str = "__bevy_wechat_";

#[derive(Debug, Clone, Copy, Default)]
pub struct SafeArea {
    /// 安全区距离屏幕边缘的距离，单位是 **CSS px**（不是画布像素）。
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub screen_width: f32,
    #[allow(dead_code)]
    pub screen_height: f32,
    /// 右上角胶囊（菜单按钮）的底边，CSS px；没有胶囊时为 0。
    pub menu_bottom: f32,
    /// 状态栏高度，CSS px（`inset_top` 已经包含它，这里保留原始值供调试）。
    #[allow(dead_code)]
    pub status_bar: f32,
}

/// 由宿主画布/安全区推导出的世界坐标系布局。
///
/// 相机始终把宽度固定为 [`DESIGN_WIDTH`]，高度跟随画布宽高比；因此高屏手机
/// 会获得更高（而不是被拉伸）的游戏区域。所有距离都在“世界像素”里。
#[derive(Resource, Debug, Clone, Copy)]
pub struct View {
    /// 世界坐标宽度，恒为 720。
    pub width: f32,
    /// 世界坐标高度 = 720 * canvas_h / canvas_w。
    pub height: f32,
    /// 安全区相对屏幕边缘的内缩，已换算到世界坐标。
    pub inset_left: f32,
    pub inset_top: f32,
    pub inset_right: f32,
    pub inset_bottom: f32,
    /// 真实画布尺寸（用于把指针位置换算到世界坐标）。
    pub canvas_w: f32,
    pub canvas_h: f32,
    /// 画布像素 → 世界像素的比例：`DESIGN_WIDTH / canvas_w`。
    pub px_to_world: f32,
}

impl View {
    /// 把指针（画布像素，左上角原点）换算成世界坐标（中心原点，y 向上）。
    pub fn pointer(&self, canvas_pos: Vec2) -> Vec2 {
        Vec2::new(
            (canvas_pos.x - self.canvas_w * 0.5) * self.px_to_world,
            (self.canvas_h * 0.5 - canvas_pos.y) * self.px_to_world,
        )
    }

    fn from_parts(canvas_w: f32, canvas_h: f32, safe: SafeArea) -> Self {
        let canvas_w = canvas_w.max(1.0);
        let canvas_h = canvas_h.max(1.0);
        let px_to_world = DESIGN_WIDTH / canvas_w;
        let screen_w = if safe.screen_width > 1.0 {
            safe.screen_width
        } else {
            canvas_w
        };
        let css_to_world = DESIGN_WIDTH / screen_w;
        // 顶部要让开胶囊按钮：安全区 top 和胶囊底边取较大者。
        let top_css = safe.top.max(safe.menu_bottom);
        Self {
            width: DESIGN_WIDTH,
            height: canvas_h * px_to_world,
            inset_left: safe.left * css_to_world,
            inset_top: top_css * css_to_world,
            inset_right: safe.right * css_to_world,
            inset_bottom: safe.bottom * css_to_world,
            canvas_w,
            canvas_h,
            px_to_world,
        }
    }
}

/// 宿主桥。字段全部是普通 Rust 值，所以作为 Bevy `Resource` 没有线程问题。
#[derive(Resource, Debug, Clone)]
pub struct Host {
    canvas_w: f32,
    canvas_h: f32,
    safe: SafeArea,
    version: String,
    viewport_seq: u32,
    device_pixel_ratio: f32,
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

impl Host {
    /// 读取适配层发布的宿主信息。非 wasm 构建返回 720×1280 的桌面默认值，
    /// 这样 `cargo check` / IDE 补全不需要目标设备。
    pub fn new() -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            // 0.3 起优先读聚合后的 `Layout`（canvas + safeArea + screen +
            // devicePixelRatio + seq）。`Canvas`/`SafeArea` 仍然发布，作为
            // 兼容 0.2 的退路。
            let layout = host_global("Layout");
            let viewport_seq = number(&layout, "seq").unwrap_or(0.0) as u32;
            let dpr = number(&layout, "devicePixelRatio").unwrap_or(1.0);

            let canvas = choose_object(&layout, "canvas", "Canvas");
            let canvas_w = number(&canvas, "width").unwrap_or(DESIGN_WIDTH);
            let canvas_h = number(&canvas, "height").unwrap_or(1280.0);

            let area = choose_object(&layout, "safeArea", "SafeArea");
            let menu = js_sys::Reflect::get(&area, &JsValue::from_str("menuButton"))
                .unwrap_or(JsValue::UNDEFINED);
            let screen = js_sys::Reflect::get(&layout, &JsValue::from_str("screen"))
                .unwrap_or(JsValue::UNDEFINED);
            let safe = SafeArea {
                left: number(&area, "left").unwrap_or(0.0),
                top: number(&area, "top").unwrap_or(0.0),
                right: number(&area, "right").unwrap_or(0.0),
                bottom: number(&area, "bottom").unwrap_or(0.0),
                screen_width: number(&area, "screenWidth")
                    .or_else(|| number(&screen, "width"))
                    .unwrap_or(canvas_w),
                screen_height: number(&area, "screenHeight")
                    .or_else(|| number(&screen, "height"))
                    .unwrap_or(canvas_h),
                menu_bottom: number(&menu, "bottom").unwrap_or(0.0),
                status_bar: number(&area, "statusBarHeight").unwrap_or(0.0),
            };

            Self {
                canvas_w,
                canvas_h,
                safe,
                version: host_global("Version")
                    .as_string()
                    .unwrap_or_else(|| "unknown".to_string()),
                viewport_seq,
                device_pixel_ratio: dpr,
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            Self {
                canvas_w: DESIGN_WIDTH,
                canvas_h: 1280.0,
                safe: SafeArea {
                    screen_width: DESIGN_WIDTH,
                    screen_height: 1280.0,
                    ..default()
                },
                version: "dev".to_string(),
                viewport_seq: 0,
                device_pixel_ratio: 1.0,
            }
        }
    }

    pub fn canvas(&self) -> (u32, u32) {
        (self.canvas_w as u32, self.canvas_h as u32)
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// 画布/安全区变化序号；启动后如果适配层再次 resize，这个值会增长。
    pub fn viewport_seq(&self) -> u32 {
        self.viewport_seq
    }

    pub fn device_pixel_ratio(&self) -> f32 {
        self.device_pixel_ratio
    }

    pub fn view(&self) -> View {
        View::from_parts(self.canvas_w, self.canvas_h, self.safe)
    }

    /// 读取系统主题（`"dark"` / `"light"`）。
    pub fn theme(&self) -> Option<Theme> {
        #[cfg(target_arch = "wasm32")]
        {
            host_global("Theme")
                .as_string()
                .and_then(|s| Theme::parse(&s))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            None
        }
    }

    /// 适配层当前是否处于“手动主题”模式。
    #[allow(dead_code)]
    pub fn theme_manual(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            host_global("ThemeManual").as_bool().unwrap_or(false)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            false
        }
    }

    /// 覆盖系统主题：0.3 起这个选择会一直保留，直到显式 `FollowTheme()`。
    pub fn set_theme(&self, theme: Theme) {
        #[cfg(target_arch = "wasm32")]
        {
            let set = host_global("SetTheme");
            if let Ok(set) = set.dyn_into::<js_sys::Function>() {
                let _ = set.call1(&JsValue::UNDEFINED, &JsValue::from_str(theme.as_str()));
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = theme;
    }

    /// 恢复跟随系统主题（清除手动覆盖）。
    #[allow(dead_code)]
    pub fn follow_theme(&self) {
        #[cfg(target_arch = "wasm32")]
        {
            let follow = host_global("FollowTheme");
            if let Ok(follow) = follow.dyn_into::<js_sys::Function>() {
                let _ = follow.call0(&JsValue::UNDEFINED);
            }
        }
    }

    /// 请求一次激励视频。返回 ticket；0 表示适配层正忙或拉起失败。
    ///
    /// 0.3 起优先用 `RequestAd` 拿到票据，再等待 `AdResult` 里同一张票据的
    /// 最终状态，避免“上一个广告的 close 回调”误触发本次奖励。
    pub fn request_ad(&self) -> u32 {
        #[cfg(target_arch = "wasm32")]
        {
            let request = host_global("RequestAd");
            if let Ok(request) = request.dyn_into::<js_sys::Function>() {
                return request
                    .call1(&JsValue::UNDEFINED, &JsValue::from_str("rewarded"))
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as u32;
            }
            0
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            0
        }
    }

    /// 适配层最近一次广告的最终结果；进行中时为 `None`。
    pub fn ad_result(&self) -> Option<AdResult> {
        #[cfg(target_arch = "wasm32")]
        {
            let value = host_global("AdResult");
            if !value.is_object() {
                return None;
            }
            Some(AdResult {
                ticket: number(&value, "ticket").unwrap_or(0.0) as u32,
                status: string(&value, "status").unwrap_or_default(),
                reason: string(&value, "reason").unwrap_or_default(),
                is_ended: js_sys::Reflect::get(&value, &JsValue::from_str("isEnded"))
                    .ok()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                mock: js_sys::Reflect::get(&value, &JsValue::from_str("mock"))
                    .ok()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            })
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            None
        }
    }

    /// 适配层是否启用了本地 mock 广告（`Config.mockAds`，不是单次结果）。
    pub fn ad_mock(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            let config = host_global("Config");
            js_sys::Reflect::get(&config, &JsValue::from_str("mockAds"))
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            false
        }
    }

    /// 最近一次广告错误原因，例如 `timeout` / `show-failed`。
    pub fn ad_last_error(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            host_global("AdLastError").as_string()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            None
        }
    }

    /// 前台/后台状态：`foreground` / `background`。
    pub fn app_state(&self) -> String {
        #[cfg(target_arch = "wasm32")]
        {
            host_global("AppState")
                .as_string()
                .unwrap_or_else(|| "foreground".to_string())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            "foreground".to_string()
        }
    }

    /// 激励视频状态：`idle/loading/ended/skipped/error/unavailable`。
    pub fn ad_status(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            host_global("AdStatus").as_string()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            None
        }
    }

    /// 读取整数存档。适配层的 `localStorage` 落在 `wx.setStorageSync` 上，
    /// 重开小游戏仍然存在。
    pub fn load_u32(&self, key: &str) -> Option<u32> {
        self.load_str(key)?.parse().ok()
    }

    pub fn save_u32(&self, key: &str, value: u32) {
        self.save_str(key, &value.to_string());
    }

    pub fn load_str(&self, key: &str) -> Option<String> {
        let value = storage_call("getItem", &JsValue::from_str(key), &JsValue::UNDEFINED);
        value.and_then(|v| v.as_string())
    }

    pub fn save_str(&self, key: &str, value: &str) {
        storage_call(
            "setItem",
            &JsValue::from_str(key),
            &JsValue::from_str(value),
        );
    }
}

/// 适配层 0.3 的广告最终结果。`ticket` 必须和本次请求的票据一致才算数。
#[derive(Debug, Clone)]
pub struct AdResult {
    pub ticket: u32,
    pub status: String,
    pub reason: String,
    pub is_ended: bool,
    pub mock: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

impl Theme {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    pub fn flipped(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }
}

// ── wasm 侧的小工具 ────────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, JsValue};

#[cfg(target_arch = "wasm32")]
fn js_window() -> JsValue {
    let global = js_sys::global();
    if let Ok(env) = js_sys::Reflect::get(&global, &JsValue::from_str("__bevy_wechat_env")) {
        if env.is_object() {
            if let Ok(win) = js_sys::Reflect::get(&env, &JsValue::from_str("window")) {
                if win.is_object() {
                    return win;
                }
            }
        }
    }
    js_sys::Reflect::get(&global, &JsValue::from_str("window")).unwrap_or(JsValue::UNDEFINED)
}

/// 读取适配层发布的全局，例如 `host_global("Canvas")`。
#[cfg(target_arch = "wasm32")]
fn host_global(name: &str) -> JsValue {
    let key = format!("{GLOBAL_PREFIX}{name}");
    js_sys::Reflect::get(&js_window(), &JsValue::from_str(&key)).unwrap_or(JsValue::UNDEFINED)
}

#[cfg(target_arch = "wasm32")]
fn number(obj: &JsValue, key: &str) -> Option<f32> {
    js_sys::Reflect::get(obj, &JsValue::from_str(key))
        .ok()
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
}

/// 优先从 `Layout` 里取嵌套对象，取不到再读旧的扁平全局。
#[cfg(target_arch = "wasm32")]
fn choose_object(parent: &JsValue, key: &str, fallback: &str) -> JsValue {
    let nested = js_sys::Reflect::get(parent, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED);
    if nested.is_object() {
        nested
    } else {
        host_global(fallback)
    }
}

#[cfg(target_arch = "wasm32")]
fn string(obj: &JsValue, key: &str) -> Option<String> {
    js_sys::Reflect::get(obj, &JsValue::from_str(key))
        .ok()
        .and_then(|v| v.as_string())
}

/// 调用 `window.localStorage.<method>(a, b)`。
#[cfg(target_arch = "wasm32")]
fn storage_call(method: &str, a: &JsValue, b: &JsValue) -> Option<JsValue> {
    let storage = js_sys::Reflect::get(&js_window(), &JsValue::from_str("localStorage")).ok()?;
    if !storage.is_object() {
        return None;
    }
    let func = js_sys::Reflect::get(&storage, &JsValue::from_str(method)).ok()?;
    let func: js_sys::Function = func.dyn_into().ok()?;
    func.call2(&storage, a, b).ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn storage_call(_method: &str, _a: &JsValue, _b: &JsValue) -> Option<JsValue> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
use fake_js::JsValue;

/// 桌面构建只需要让类型存在；真正的实现永远走不到。
#[cfg(not(target_arch = "wasm32"))]
mod fake_js {
    #[derive(Debug, Clone, Default)]
    pub struct JsValue;

    impl JsValue {
        pub const UNDEFINED: Self = Self;
        pub fn from_str(_: &str) -> Self {
            Self
        }
        pub fn as_string(&self) -> Option<String> {
            None
        }
    }
}
