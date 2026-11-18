//! Bevy 0.20 × 微信小游戏：Pong 示例。
//!
//! 这个 crate 是“平台薄壳”：它只做三件事
//!
//! 1. 从适配层发布的全局里读出画布尺寸 / 安全区 / 主题（`host.rs`）；
//! 2. 用这些尺寸创建 winit 窗口和正交相机（`main.rs` + `game.rs::setup`）；
//! 3. 跑游戏逻辑（`game.rs`）。
//!
//! 所有 `wx.*` 调用都在 `addons/bevy-wechat/runtime/adapter.js` 里，游戏侧不
//! 直接依赖微信 SDK。构建入口是仓库根目录的 `./build.sh`。

mod game;
mod host;
mod probe;

use bevy::prelude::*;
use bevy::render::error_handler::{RenderErrorHandler, RenderErrorPolicy};
use bevy::window::{CompositeAlphaMode, PrimaryWindow, WindowResolution};
use bevy::winit::WinitSettings;

use crate::host::Host;

fn main() {
    // 真机上 `panic = "abort"` 的控制台输出经常被吞掉，必须主动弹窗。
    probe::install_panic_hook();

    let host = Host::new();
    let view = host.view();
    let (canvas_w, canvas_h) = host.canvas();

    let mut app = App::new();
    app.insert_resource(ClearColor(Color::BLACK))
        .insert_resource(host)
        .insert_resource(view)
        // 真机页面不会收到 focus 事件，默认的 reactive 模式会在几帧后停住。
        .insert_resource(WinitSettings::continuous())
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Bevy Pong - WeChat".into(),
                canvas: None,
                resolution: WindowResolution::new(canvas_w, canvas_h),
                resizable: false,
                ..default()
            }),
            ..default()
        }))
        // WebGL2 上任何一次 wgpu 错误默认都会停掉渲染，表现为“第一帧后黑屏”。
        // 这里只记录，继续渲染。
        .insert_resource(RenderErrorHandler(|error, _main, _render| {
            error!("RenderError {:?}: {}", error.ty, error.description);
            RenderErrorPolicy::Ignore
        }))
        .add_systems(Startup, force_opaque_window);

    game::build(&mut app);
    probe::install(&mut app);
    app.run();
}

/// WebGL 画布默认按预乘 alpha 合成；如果帧缓冲 alpha 是 0，手机就会看到黑色。
fn force_opaque_window(mut windows: Query<&mut Window, With<PrimaryWindow>>) {
    for mut window in &mut windows {
        window.composite_alpha_mode = CompositeAlphaMode::Opaque;
    }
}
