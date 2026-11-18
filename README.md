# Bevy Pong × 微信小游戏

一个竖屏 Pong，用 Bevy 0.20 写成，通过
[bevy-wechat-adapter](https://gitee.com/pppkin/bevy_wechat) 跑进微信小游戏。

它是一份最小的配套示例：除游戏逻辑外，只有画布尺寸、安全区、胶囊避让、指针换算、
主题、激励视频、存档和真机排错的胶水代码。照它走一遍，就能把同样的步骤搬到你自己的
Bevy 游戏里。

截图来自微信开发者工具模拟器，画布为 720×1561。真机尺寸通常不同，游戏会自动适配。

## 目录

```text
assets/audio/            内嵌到 wasm 的音效（include_bytes!，不进包）
platforms/wechat/
  Cargo.toml             平台 crate：bevy + 宿主桥，唯一的构建目标
  src/main.rs            入口：建窗口、装插件、跑游戏
  src/host.rs            读适配层发布的全局（尺寸 / 安全区 / 主题 / 广告 / 存档）
  src/game.rs            Pong 本身，不直接调用任何 wx.*
  src/probe.rs           panic 桥与可选诊断
  package/               构建产物，整包由 ./build.sh 生成（已 gitignore）
build.sh                 把本仓库的目录布局翻译成适配层参数
addons/bevy-wechat/      适配层 submodule
```

## 安装

### 1. 克隆仓库

```bash
git clone --recurse-submodules https://gitee.com/pppkin/bevy_wechat_example.git
cd bevy_wechat_example
```

如果已经克隆过，补上 submodule：

```bash
git submodule update --init --recursive
```

### 2. 安装工具

| 工具 | 要求 |
| --- | --- |
| Rust | stable；`rust-toolchain.toml` 会自动装 wasm32 目标 |
| `wasm-bindgen` CLI | 必须 0.2.129，和 `Cargo.toml` 一致 |
| `wasm-opt` | Binaryen，实测 133 |
| `brotli` | 生成 `.wasm.br` |
| `python3` | 运行 glue 补丁 |
| `esbuild` | 可选；有就压缩 JS |
| 微信开发者工具 | 导入 `platforms/wechat/package/` |

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
brew install binaryen brotli
brew install esbuild        # 可选
```

### 3. 构建

微信小游戏必须绑定你自己的小游戏 AppID（`touristappid` 对小游戏不生效）：

```bash
WECHAT_APPID=wx1234567890abcdef ./build.sh
```

首次约 3 分钟，之后增量约 1 分钟。构建完成后 `platforms/wechat/package/` 里是：

```text
game.js  game.json  project.config.json  audio.js   由模板生成
pong.js  pong.wasm.br                               wasm 与 glue
runtime/                                            wx.* 适配层
```

### 4. 在微信开发者工具里打开

项目 → 导入项目 → 目录选 `platforms/wechat/package`，AppID 填构建时用的那个。
开发者工具会改写 `project.config.json`，重新构建会恢复成模板生成的内容。

## 操作

- 在球场上按住并拖动：控制右侧蓝色球拍（`Touches` / 鼠标光标都可以）。
- 左上角方块：切换深色 / 浅色主题，选择会写进 `wx` 存档。
- 底部橙色长条：模拟激励视频，结束后下一次接球加速。

## 这个示例演示了什么

- **平台隔离**：`game.rs` 只有 Bevy；所有 `wx.*` 调用都在
  `addons/bevy-wechat/runtime/adapter.js` 里，对游戏暴露成一个浏览器形状的
  shim（canvas / window / localStorage / PointerEvent / rAF）。
- **宿主桥**：`host.rs` 通过 `__bevy_wechat_*` 全局读画布、安全区、胶囊、主题、
  广告票据和前台状态，细节见适配层 README 的「宿主契约」。
- **安全区与胶囊**：相机固定 720 宽、高度跟随画布比例，HUD 让开状态栏与右上角
  胶囊按钮。
- **存档**：`localStorage` 落在 `wx.setStorageSync` 上，重开小游戏仍在。
- **音频**：三个 wav 用 `include_bytes!` 内嵌，经 `bevy_audio` 播放；示例刻意
  不用文字，因此没有字体资源，也不需要 `bevy_text`。
- **错误可视化**：`panic` 通过桥接弹出 `wx.showModal`；真机日志被吞时仍能看到
  原因。

## 移植时容易踩的坑

1. **不要设 `canvas` 选择器**：小游戏没有 DOM，`window.canvas` 留 `None` 才能让
   winit 用适配层包装的主画布。
2. **没有 `eval`**：wasm 侧读开关、弹窗都要走 `Reflect` + 桥接函数，不能拼 JS
   字符串再 `eval`。
3. **音频对象要预热**：`wx.createInnerAudioContext` 首次播放要几十毫秒，放在战斗
   帧里会直接掉帧。适配层的 `audio.js` 在启动时分帧建池。
4. **WebGL 后端的怪癖**（ASTC 空壳、状态查询返回数字、GLSL 拒绝 `smooth`）已在
   适配层里修好，业务代码不用管；排查时看适配层 README 的「平台差异」。

## 许可

WTFPL v2。适配层 submodule 采用同一许可。
