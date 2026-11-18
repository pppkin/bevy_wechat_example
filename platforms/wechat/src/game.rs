//! Pong 的全部玩法、表现和少量平台交互。
//!
//! 所有图形都是纯色 `Sprite`：示例不打包任何资源，构建产物只有 wasm + JS。
//! 分数用 7 个“计分块”表示；顶部中央的菱形在追平/刷新纪录时点亮。

use bevy::audio::{AudioPlayer, AudioSource, PlaybackSettings};
use bevy::camera::ScalingMode;
use bevy::prelude::*;
use bevy::time::Virtual;
use bevy::window::PrimaryWindow;
use std::sync::Arc;

use crate::host::{Host, Theme, View, DESIGN_WIDTH};

const WIN_SCORE: u32 = 7;
const PADDLE_W: f32 = 18.0;
const PADDLE_H: f32 = 132.0;
const BALL_SIZE: f32 = 22.0;
const TRAIL_LEN: usize = 5;
const MAX_SLOWMO: f32 = 8.0;
const SLOWMO_SCALE: f32 = 0.45;

/// 实际可玩区域（世界坐标），已经避开安全区和底部广告条。
#[derive(Resource, Clone, Copy)]
struct Field {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
}

impl Field {
    fn from_view(view: &View) -> Self {
        let half_w = view.width * 0.5;
        let half_h = view.height * 0.5;
        Self {
            left: -half_w + view.inset_left + 28.0,
            right: half_w - view.inset_right - 28.0,
            top: half_h - view.inset_top - 86.0,
            bottom: -half_h + view.inset_bottom + 118.0,
        }
    }

    fn paddle_x(&self, side: Side) -> f32 {
        match side {
            Side::Ai => self.left + 26.0,
            Side::Player => self.right - 26.0,
        }
    }

    fn clamp_paddle(&self, y: f32) -> f32 {
        y.clamp(
            self.bottom + PADDLE_H * 0.5,
            self.top - PADDLE_H * 0.5,
        )
    }

    fn chip_y(&self) -> f32 {
        self.bottom - 52.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Ai,
    Player,
}

impl Side {
    fn index(self) -> usize {
        match self {
            Self::Ai => 0,
            Self::Player => 1,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Serve(f32),
    Rally,
    Over { timer: f32, winner: Side },
}

#[derive(Resource)]
struct Match {
    score: [u32; 2],
    wins: [u32; 2],
    rally: u32,
    best_rally: u32,
    phase: Phase,
    /// 下一次发球的方向：-1 朝左（AI），+1 朝右（玩家）。
    serve_dir: f32,
    /// 0..1，得分/获胜时的边框闪光。
    flash: f32,
}

impl Match {
    fn load(host: &Host) -> Self {
        Self {
            score: [0, 0],
            wins: [
                host.load_u32("wins.ai").unwrap_or(0),
                host.load_u32("wins.player").unwrap_or(0),
            ],
            rally: 0,
            best_rally: host.load_u32("best_rally").unwrap_or(0),
            phase: Phase::Serve(1.2),
            serve_dir: 1.0,
            flash: 0.0,
        }
    }
}

#[derive(Resource)]
struct Rng(u32);

impl Rng {
    fn next_f32(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / 16_777_216.0
    }
}

#[derive(Resource, Default)]
struct Pointer {
    last: Option<Vec2>,
    idle: f32,
}

#[derive(Resource, Default)]
struct Effects {
    /// 剩余慢动作时间（真实秒）。
    slowmo: f32,
}

#[derive(Resource)]
struct Ads {
    phase: AdPhase,
    /// 本次请求的票据；只接受同一张票据的 `AdResult`。
    ticket: u32,
}

#[derive(Clone, Copy, Debug)]
enum AdPhase {
    Idle,
    /// 已拉起激励视频，等待 `AdStatus` 变化。
    Pending(f32),
    /// 没有广告位（或拉起失败）时的本地 mock。
    Mock(f32),
    Error(f32),
}

impl Default for Ads {
    fn default() -> Self {
        Self {
            phase: AdPhase::Idle,
            ticket: 0,
        }
    }
}

#[derive(Resource)]
struct Look {
    theme: Theme,
    /// 玩家是否手动覆盖了系统主题（会持久化）。
    custom: bool,
}

impl Look {
    fn load(host: &Host) -> Self {
        let saved = host.load_str("theme").as_deref().and_then(Theme::parse);
        Self {
            theme: saved.or_else(|| host.theme()).unwrap_or(Theme::Dark),
            custom: saved.is_some(),
        }
    }

    fn palette(&self) -> Palette {
        Palette::of(self.theme)
    }
}

#[derive(Clone, Copy)]
enum Role {
    Frame,
    Line,
    Player,
    Ai,
    Ball,
    PipOff,
    Chip,
    ChipIcon,
    Accent,
    Record,
}

#[derive(Component)]
struct Themed {
    role: Role,
    alpha: f32,
}

#[derive(Component)]
struct Ball {
    vel: Vec2,
    history: [Vec2; TRAIL_LEN],
}

#[derive(Component)]
struct Trail(usize);

#[derive(Component)]
struct Paddle(Side);

#[derive(Component)]
struct ScorePip {
    side: Side,
    index: u32,
}

#[derive(Component)]
struct AdChip;

#[derive(Component)]
struct AdProgress;

#[derive(Component)]
struct ThemeButton;

/// 音效直接编译进 wasm，不经过资源服务器。
///
/// 这样示例在小游戏环境里不依赖 `fetch` / 文件系统，也能验证
/// `bevy_audio` → rodio → cpal → WebAudio 这条完整链路。
#[derive(Resource, Clone)]
struct Sfx {
    hit: Handle<AudioSource>,
    wall: Handle<AudioSource>,
    score: Handle<AudioSource>,
}

fn play_sfx(commands: &mut Commands, handle: &Handle<AudioSource>) {
    commands.spawn((AudioPlayer(handle.clone()), PlaybackSettings::DESPAWN));
}

struct Palette {
    bg: Color,
    frame: Color,
    line: Color,
    player: Color,
    ai: Color,
    ball: Color,
    pip_off: Color,
    chip: Color,
    chip_icon: Color,
    accent: Color,
    warn: Color,
}

impl Palette {
    fn of(theme: Theme) -> Self {
        match theme {
            Theme::Dark => Self {
                bg: Color::srgb(0.035, 0.047, 0.086),
                frame: Color::srgb(0.145, 0.196, 0.310),
                line: Color::srgb(0.196, 0.259, 0.392),
                player: Color::srgb(0.224, 0.827, 1.000),
                ai: Color::srgb(1.000, 0.365, 0.561),
                ball: Color::srgb(1.000, 0.820, 0.400),
                pip_off: Color::srgb(0.180, 0.227, 0.345),
                chip: Color::srgb(1.000, 0.690, 0.125),
                chip_icon: Color::srgb(0.090, 0.070, 0.030),
                accent: Color::srgb(0.486, 0.949, 0.784),
                warn: Color::srgb(1.000, 0.302, 0.302),
            },
            Theme::Light => Self {
                bg: Color::srgb(0.933, 0.949, 0.972),
                frame: Color::srgb(0.765, 0.808, 0.878),
                line: Color::srgb(0.718, 0.769, 0.855),
                player: Color::srgb(0.000, 0.475, 0.722),
                ai: Color::srgb(0.839, 0.200, 0.424),
                ball: Color::srgb(0.769, 0.478, 0.000),
                pip_off: Color::srgb(0.816, 0.855, 0.914),
                chip: Color::srgb(0.878, 0.541, 0.000),
                chip_icon: Color::srgb(1.000, 0.965, 0.878),
                accent: Color::srgb(0.059, 0.616, 0.463),
                warn: Color::srgb(0.776, 0.157, 0.157),
            },
        }
    }

    fn color(&self, role: Role) -> Color {
        match role {
            Role::Frame => self.frame,
            Role::Line => self.line,
            Role::Player => self.player,
            Role::Ai => self.ai,
            Role::Ball => self.ball,
            Role::PipOff => self.pip_off,
            Role::Chip => self.chip,
            Role::ChipIcon => self.chip_icon,
            Role::Accent => self.accent,
            Role::Record => self.ball,
        }
    }
}

/// 组装 Pong：加载存档、插入资源、注册启动和每帧系统。
pub fn build(app: &mut App) {
    let view = *app.world().resource::<View>();
    let host = app.world().resource::<Host>();
    let look = Look::load(host);
    let game = Match::load(host);
    let field = Field::from_view(&view);

    if look.custom {
        // 存档里保存了手动主题；0.3 的手动选择会一直生效，启动时同步给适配层。
        host.set_theme(look.theme);
    }

    info!(
        "pong: adapter {} (dpr {}, viewport seq {}, ad mock {})",
        host.version(),
        host.device_pixel_ratio(),
        host.viewport_seq(),
        host.ad_mock(),
    );
    info!(
        "pong: canvas {}x{} -> world {}x{}, safe insets {:.1}/{:.1}/{:.1}/{:.1}, best rally {}, wins AI {} / player {}",
        view.canvas_w,
        view.canvas_h,
        view.width,
        view.height,
        view.inset_left,
        view.inset_top,
        view.inset_right,
        view.inset_bottom,
        game.best_rally,
        game.wins[0],
        game.wins[1],
    );

    app.insert_resource(look)
        .insert_resource(game)
        .insert_resource(field)
        .insert_resource(Rng(0x5eed_1234))
        .insert_resource(Pointer::default())
        .insert_resource(Effects::default())
        .insert_resource(Ads::default())
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                update_player,
                update_ai,
                step_ball,
                handle_taps,
                sync_environment,
                tick_effects,
                paint_scene,
            )
                .chain(),
        );
}

fn setup(
    mut commands: Commands,
    mut assets: ResMut<Assets<AudioSource>>,
    view: Res<View>,
    field: Res<Field>,
) {
    commands.insert_resource(Sfx {
        hit: assets.add(AudioSource {
            bytes: Arc::from(&include_bytes!("../../../assets/audio/hit.wav")[..]),
        }),
        wall: assets.add(AudioSource {
            bytes: Arc::from(&include_bytes!("../../../assets/audio/wall.wav")[..]),
        }),
        score: assets.add(AudioSource {
            bytes: Arc::from(&include_bytes!("../../../assets/audio/score.wav")[..]),
        }),
    });

    let camera = commands
        .spawn((
            Camera2d,
            Projection::Orthographic(OrthographicProjection {
                scaling_mode: ScalingMode::Fixed {
                    width: DESIGN_WIDTH,
                    height: view.height,
                },
                ..OrthographicProjection::default_2d()
            }),
        ))
        .id();
    // WebGL2 后端无法创建多重采样管线：Camera2d 自带 Msaa，必须覆盖而不是追加。
    commands.entity(camera).insert(Msaa::Off);

    let width = field.right - field.left;
    let height = field.top - field.bottom;
    let cx = (field.left + field.right) * 0.5;
    let cy = (field.top + field.bottom) * 0.5;

    // 场地边框：四条细线。
    spawn(&mut commands, Role::Frame, 0.8, Vec2::new(width, 3.0), vec3(cx, field.top, 0.5));
    spawn(&mut commands, Role::Frame, 0.8, Vec2::new(width, 3.0), vec3(cx, field.bottom, 0.5));
    spawn(&mut commands, Role::Frame, 0.6, Vec2::new(3.0, height), vec3(field.left, cy, 0.5));
    spawn(&mut commands, Role::Frame, 0.6, Vec2::new(3.0, height), vec3(field.right, cy, 0.5));

    // 中线虚线。
    let mut y = field.bottom + 24.0;
    while y < field.top - 24.0 {
        spawn(&mut commands, Role::Line, 0.35, Vec2::new(5.0, 26.0), vec3(0.0, y, 0.4));
        y += 64.0;
    }

    // 计分块：左半场是 AI，右半场是玩家。
    let pip_y = field.top + 36.0;
    for i in 0..WIN_SCORE {
        let offset = 78.0 + i as f32 * 28.0;
        for side in [Side::Ai, Side::Player] {
            let dir = match side {
                Side::Ai => -1.0,
                Side::Player => 1.0,
            };
            commands.spawn((
                Sprite::from_color(Color::WHITE, Vec2::new(20.0, 10.0)),
                Transform::from_xyz(dir * offset, pip_y, 0.9),
                Themed {
                    role: Role::PipOff,
                    alpha: 0.9,
                },
                ScorePip { side, index: i },
            ));
        }
    }

    // 顶部中央的纪录菱形。
    commands.spawn((
        Sprite::from_color(Color::WHITE, Vec2::splat(15.0)),
        Transform::from_xyz(0.0, pip_y, 0.9).with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4)),
        Themed {
            role: Role::Record,
            alpha: 1.0,
        },
    ));

    // 主题切换按钮（左上角）。
    commands.spawn((
        Sprite::from_color(Color::WHITE, Vec2::splat(46.0)),
        Transform::from_xyz(field.left + 24.0, pip_y, 0.9),
        Themed {
            role: Role::Frame,
            alpha: 0.9,
        },
        ThemeButton,
    ));
    commands.spawn((
        Sprite::from_color(Color::WHITE, Vec2::splat(22.0)),
        Transform::from_xyz(field.left + 24.0, pip_y, 1.0),
        Themed {
            role: Role::Accent,
            alpha: 1.0,
        },
        ThemeButton,
    ));

    // 球拍：AI 在左、玩家在右。
    for side in [Side::Ai, Side::Player] {
        commands.spawn((
            Sprite::from_color(Color::WHITE, Vec2::new(PADDLE_W, PADDLE_H)),
            Transform::from_xyz(field.paddle_x(side), 0.0, 2.0),
            Themed {
                role: match side {
                    Side::Ai => Role::Ai,
                    Side::Player => Role::Player,
                },
                alpha: 1.0,
            },
            Paddle(side),
        ));
    }

    // 球与拖尾。
    let history = [Vec2::ZERO; TRAIL_LEN];
    commands.spawn((
        Sprite::from_color(Color::WHITE, Vec2::splat(BALL_SIZE)),
        Transform::from_xyz(0.0, 0.0, 3.0),
        Themed {
            role: Role::Ball,
            alpha: 1.0,
        },
        Ball {
            vel: Vec2::ZERO,
            history,
        },
    ));
    for i in 0..TRAIL_LEN {
        let size = BALL_SIZE - 3.0 * i as f32;
        commands.spawn((
            Sprite::from_color(Color::WHITE, Vec2::splat(size)),
            Transform::from_xyz(0.0, 0.0, 2.5),
            Themed {
                role: Role::Ball,
                alpha: 0.30 - 0.055 * i as f32,
            },
            Trail(i),
        ));
    }

    // 底部广告条：整块是按钮，中间菱形是图标，下方细条是慢动作剩余时间。
    let chip_y = field.chip_y();
    commands.spawn((
        Sprite::from_color(Color::WHITE, Vec2::new(200.0, 58.0)),
        Transform::from_xyz(0.0, chip_y, 2.0),
        Themed {
            role: Role::Chip,
            alpha: 1.0,
        },
        AdChip,
    ));
    commands.spawn((
        Sprite::from_color(Color::WHITE, Vec2::splat(20.0)),
        Transform::from_xyz(0.0, chip_y, 2.1).with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4)),
        Themed {
            role: Role::ChipIcon,
            alpha: 1.0,
        },
        AdChip,
    ));
    commands.spawn((
        Sprite::from_color(Color::WHITE, Vec2::new(200.0, 5.0)),
        Transform::from_xyz(0.0, chip_y - 40.0, 2.0).with_scale(Vec3::new(0.001, 1.0, 1.0)),
        Themed {
            role: Role::Accent,
            alpha: 0.9,
        },
        AdProgress,
    ));
}

fn spawn(commands: &mut Commands, role: Role, alpha: f32, size: Vec2, translation: Vec3) {
    commands.spawn((
        Sprite::from_color(Color::WHITE, size),
        Transform::from_translation(translation),
        Themed { role, alpha },
    ));
}

fn vec3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

// ── 输入 ──────────────────────────────────────────────────────────────────────

fn update_player(
    time: Res<Time>,
    view: Res<View>,
    field: Res<Field>,
    windows: Query<&Window, With<PrimaryWindow>>,
    touches: Res<Touches>,
    keys: Res<ButtonInput<KeyCode>>,
    mut pointer: ResMut<Pointer>,
    ball: Query<&Transform, (With<Ball>, Without<Paddle>)>,
    mut paddles: Query<(&Paddle, &mut Transform), Without<Ball>>,
) {
    let dt = time.delta_secs().min(0.05);
    let mut target = None;

    if let Ok(window) = windows.single() {
        if let Some(pos) = window.cursor_position() {
            target = Some(view.pointer(pos));
        }
    }
    if let Some(touch) = touches.iter().next() {
        target = Some(view.pointer(touch.position()));
    }

    if let Some(p) = target {
        pointer.last = Some(p);
        pointer.idle = 0.0;
    } else {
        pointer.idle += dt;
    }

    let ball_y = ball.single().map(|t| t.translation.y).unwrap_or(0.0);

    let Some((_, mut transform)) = paddles.iter_mut().find(|(paddle, _)| paddle.0 == Side::Player)
    else {
        return;
    };

    let mut goal = match (target, pointer.idle) {
        (Some(p), _) => p.y,
        // 模拟器里没有指针事件时自动接球，方便截屏和观察。
        (None, idle) if idle > 1.5 => ball_y,
        _ => transform.translation.y,
    };

    if keys.pressed(KeyCode::ArrowUp) || keys.pressed(KeyCode::KeyW) {
        goal = transform.translation.y + 900.0 * dt;
    } else if keys.pressed(KeyCode::ArrowDown) || keys.pressed(KeyCode::KeyS) {
        goal = transform.translation.y - 900.0 * dt;
    }

    let max_step = 3200.0 * dt;
    let next = transform.translation.y + (goal - transform.translation.y).clamp(-max_step, max_step);
    transform.translation.y = field.clamp_paddle(next);
}

fn update_ai(
    time: Res<Time>,
    field: Res<Field>,
    game: Res<Match>,
    ball: Query<(&Transform, &Ball), Without<Paddle>>,
    mut paddles: Query<(&Paddle, &mut Transform), Without<Ball>>,
) {
    let dt = time.delta_secs().min(0.05);
    let Ok((ball_transform, ball)) = ball.single() else {
        return;
    };
    let Some((_, mut transform)) = paddles.iter_mut().find(|(paddle, _)| paddle.0 == Side::Ai)
    else {
        return;
    };

    let target_x = field.paddle_x(Side::Ai);
    let predicted = if ball.vel.x < -1.0 {
        predict_y(ball_transform.translation.truncate(), ball.vel, target_x, field.top, field.bottom)
    } else {
        (field.top + field.bottom) * 0.5
    };
    // 一点摆动让 AI 不是完美机器；回合越多速度越快，但仍有上限。
    let wobble = (time.elapsed_secs() * 1.7).sin() * 16.0;
    let speed = (520.0 + (game.score[0] + game.score[1]) as f32 * 18.0 + game.rally as f32 * 10.0)
        .min(980.0);
    let goal = field.clamp_paddle(predicted + wobble);
    let max_step = speed * dt;
    transform.translation.y += (goal - transform.translation.y).clamp(-max_step, max_step);
}

/// 球在两面墙之间反弹后，到达 `target_x` 时的 y 坐标。
fn predict_y(pos: Vec2, vel: Vec2, target_x: f32, top: f32, bottom: f32) -> f32 {
    let t = (target_x - pos.x) / vel.x;
    if t < 0.0 {
        return pos.y;
    }
    let span = top - bottom;
    if span <= 1.0 {
        return pos.y;
    }
    let period = span * 2.0;
    let mut local = (pos.y + vel.y * t - bottom) % period;
    if local < 0.0 {
        local += period;
    }
    let offset = if local > span { period - local } else { local };
    bottom + offset
}

// ── 球与比分 ──────────────────────────────────────────────────────────────────

fn step_ball(
    time: Res<Time>,
    field: Res<Field>,
    host: Res<Host>,
    mut game: ResMut<Match>,
    mut rng: ResMut<Rng>,
    mut ball_query: Query<(&mut Ball, &mut Transform, &mut Visibility), Without<Trail>>,
    mut trail_query: Query<(&Trail, &mut Transform), (Without<Ball>, Without<Paddle>)>,
    paddles: Query<(&Paddle, &Transform), (Without<Ball>, Without<Trail>)>,
    sfx: Res<Sfx>,
    mut commands: Commands,
) {
    let dt = time.delta_secs().min(0.033);
    let Ok((mut ball, mut transform, mut visibility)) = ball_query.single_mut() else {
        return;
    };

    match game.phase {
        Phase::Serve(timer) => {
            *visibility = Visibility::Inherited;
            let left = timer - dt;
            let pulse = 1.0 + 0.18 * (time.elapsed_secs() * 9.0).sin().abs();
            transform.translation = vec3(0.0, 0.0, 3.0);
            transform.scale = Vec3::splat(pulse);
            ball.vel = Vec2::ZERO;
            ball.history = [Vec2::ZERO; TRAIL_LEN];

            if left <= 0.0 {
                let speed = 540.0 + (game.score[0] + game.score[1]) as f32 * 18.0;
                let vy = (rng.next_f32() * 2.0 - 1.0) * 260.0;
                ball.vel = Vec2::new(game.serve_dir, vy / speed).normalize() * speed;
                transform.scale = Vec3::ONE;
                game.phase = Phase::Rally;
            } else {
                game.phase = Phase::Serve(left);
            }
        }
        Phase::Rally => {
            *visibility = Visibility::Inherited;
            transform.scale = Vec3::ONE;

            let speed = ball.vel.length();
            let steps = ((speed * dt / 5.0).ceil() as u32).clamp(1, 16);
            let sub = dt / steps as f32;
            let mut scorer: Option<Side> = None;

            'steps: for _ in 0..steps {
                transform.translation.x += ball.vel.x * sub;
                transform.translation.y += ball.vel.y * sub;

                let half = BALL_SIZE * 0.5;
                if transform.translation.y > field.top - half {
                    transform.translation.y = field.top - half;
                    ball.vel.y = -ball.vel.y.abs();
                    play_sfx(&mut commands, &sfx.wall);
                } else if transform.translation.y < field.bottom + half {
                    transform.translation.y = field.bottom + half;
                    ball.vel.y = ball.vel.y.abs();
                    play_sfx(&mut commands, &sfx.wall);
                }

                for (paddle, paddle_transform) in &paddles {
                    let dx = (transform.translation.x - paddle_transform.translation.x).abs();
                    let dy = (transform.translation.y - paddle_transform.translation.y).abs();
                    if dx < PADDLE_W * 0.5 + half && dy < PADDLE_H * 0.5 + half {
                        let dir = match paddle.0 {
                            Side::Ai => 1.0,
                            Side::Player => -1.0,
                        };
                        if dir * ball.vel.x < 0.0 {
                            transform.translation.x = paddle_transform.translation.x
                                + dir * (PADDLE_W * 0.5 + half + 0.5);
                            let offset = ((transform.translation.y - paddle_transform.translation.y)
                                / (PADDLE_H * 0.5))
                                .clamp(-1.0, 1.0);
                            let speed = (ball.vel.length() * 1.05).min(1180.0);
                            let angle = offset * 0.85;
                            ball.vel = Vec2::new(dir * angle.cos(), angle.sin()) * speed;
                            play_sfx(&mut commands, &sfx.hit);
                            game.rally += 1;
                            if game.rally > game.best_rally {
                                game.best_rally = game.rally;
                                host.save_u32("best_rally", game.best_rally);
                                info!("pong: new best rally {}", game.best_rally);
                            }
                        }
                    }
                }

                if transform.translation.x < field.left - 40.0 {
                    scorer = Some(Side::Player);
                    break 'steps;
                }
                if transform.translation.x > field.right + 40.0 {
                    scorer = Some(Side::Ai);
                    break 'steps;
                }
            }

            // 拖尾采样。
            for i in (1..TRAIL_LEN).rev() {
                ball.history[i] = ball.history[i - 1];
            }
            ball.history[0] = transform.translation.truncate();

            if let Some(winner) = scorer {
                play_sfx(&mut commands, &sfx.score);
                game.score[winner.index()] += 1;
                game.flash = 0.55;
                game.serve_dir = if winner == Side::Player { -1.0 } else { 1.0 };
                game.rally = 0;
                ball.vel = Vec2::ZERO;
                ball.history = [Vec2::ZERO; TRAIL_LEN];
                transform.translation = vec3(0.0, 0.0, 3.0);

                if game.score[winner.index()] >= WIN_SCORE {
                    game.wins[winner.index()] += 1;
                    host.save_u32(
                        if winner == Side::Player {
                            "wins.player"
                        } else {
                            "wins.ai"
                        },
                        game.wins[winner.index()],
                    );
                    info!(
                        "pong: {:?} wins {}:{} (total {}:{}), best rally {}",
                        winner,
                        game.score[0],
                        game.score[1],
                        game.wins[0],
                        game.wins[1],
                        game.best_rally
                    );
                    game.phase = Phase::Over {
                        timer: 2.8,
                        winner,
                    };
                } else {
                    game.phase = Phase::Serve(0.85);
                }
            }
        }
        Phase::Over { timer, winner } => {
            let left = timer - dt;
            if left <= 0.0 {
                game.score = [0, 0];
                game.rally = 0;
                game.flash = 0.0;
                game.phase = Phase::Serve(1.0);
                ball.vel = Vec2::ZERO;
                ball.history = [Vec2::ZERO; TRAIL_LEN];
                transform.translation = vec3(0.0, 0.0, 3.0);
                *visibility = Visibility::Inherited;
            } else {
                *visibility = Visibility::Hidden;
                game.phase = Phase::Over { timer: left, winner };
            }
        }
    }

    // 球出界（Over）时拖尾也一起隐藏；这里只更新位置。
    for (trail, mut trail_transform) in &mut trail_query {
        let p = ball.history[trail.0];
        trail_transform.translation = vec3(p.x, p.y, 2.5);
    }
}

// ── 交互：主题切换 + 激励视频 ───────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn handle_taps(
    field: Res<Field>,
    view: Res<View>,
    host: Res<Host>,
    windows: Query<&Window, With<PrimaryWindow>>,
    touches: Res<Touches>,
    mouse: Res<ButtonInput<MouseButton>>,
    real: Res<Time<Real>>,
    mut last_tap: Local<Option<(f32, Vec2)>>,
    mut look: ResMut<Look>,
    mut ads: ResMut<Ads>,
    fx: Res<Effects>,
    sfx: Res<Sfx>,
    mut commands: Commands,
) {
    let mut taps: Vec<Vec2> = Vec::new();
    for touch in touches.iter_just_pressed() {
        taps.push(view.pointer(touch.position()));
    }
    // 触摸会同时产生触摸事件和合成鼠标事件；只认触摸，避免一次点击触发两次。
    if taps.is_empty() && mouse.just_pressed(MouseButton::Left) {
        if let Ok(window) = windows.single() {
            if let Some(pos) = window.cursor_position() {
                taps.push(view.pointer(pos));
            }
        }
    }
    if taps.is_empty() {
        return;
    }

    let pip_y = field.top + 36.0;
    let theme_center = Vec2::new(field.left + 24.0, pip_y);
    let chip_center = Vec2::new(0.0, field.chip_y());

    let now = real.elapsed_secs();
    for tap in taps {
        // 触摸的 pointerdown 与合成鼠标事件可能落在相邻两帧：同一点位 250ms
        // 内的重复“点击”只算一次，否则主题按钮会连续切换两次。
        if let Some((then, at)) = *last_tap {
            if now - then < 0.25 && (tap - at).length() < 24.0 {
                continue;
            }
        }
        *last_tap = Some((now, tap));

        if (tap - theme_center).abs().cmple(Vec2::splat(30.0)).all() {
            look.theme = look.theme.flipped();
            look.custom = true;
            play_sfx(&mut commands, &sfx.hit);
            host.set_theme(look.theme);
            host.save_str("theme", look.theme.as_str());
            info!("pong: theme -> {}", look.theme.as_str());
            continue;
        }

        if (tap - chip_center).abs().cmple(Vec2::new(110.0, 36.0)).all()
            && fx.slowmo <= 0.0
            && matches!(ads.phase, AdPhase::Idle)
        {
            let ticket = host.request_ad();
            if ticket > 0 {
                ads.ticket = ticket;
                ads.phase = AdPhase::Pending(0.0);
                info!("pong: rewarded ad requested ticket={ticket}");
            } else {
                // 适配层拒绝：正在播另一个广告，或配置了广告位但拉起失败。
                // 示例选择本地 mock，保证玩法仍可演示。
                ads.ticket = 0;
                ads.phase = AdPhase::Mock(0.9);
                info!(
                    "pong: ad request rejected ({:?} / {:?}), using local mock",
                    host.ad_status(),
                    host.ad_last_error(),
                );
            }
        }
    }
}

fn sync_environment(
    host: Res<Host>,
    mut look: ResMut<Look>,
    mut virtual_time: ResMut<Time<Virtual>>,
    mut tick: Local<u32>,
    mut last_state: Local<Option<String>>,
) {
    *tick += 1;

    // 切到后台时暂停虚拟时钟，避免回到前台时一次性补上几秒的 dt。
    let state = host.app_state();
    if last_state.as_deref() != Some(state.as_str()) {
        if state == "background" {
            virtual_time.pause();
            info!("pong: app background, virtual clock paused");
        } else if !state.is_empty() {
            virtual_time.unpause();
            info!("pong: app foreground, virtual clock resumed");
        }
        *last_state = Some(state);
    }

    if look.custom || tick.wrapping_add(1) % 30 != 0 {
        return;
    }
    if let Some(theme) = host.theme() {
        if theme != look.theme {
            look.theme = theme;
            info!("pong: system theme -> {}", theme.as_str());
        }
    }
}

fn tick_effects(
    real: Res<Time<Real>>,
    host: Res<Host>,
    mut virtual_time: ResMut<Time<Virtual>>,
    mut fx: ResMut<Effects>,
    mut ads: ResMut<Ads>,
    mut game: ResMut<Match>,
) {
    let dt = real.delta_secs().min(0.1);
    game.flash = (game.flash - dt * 3.0).max(0.0);

    if fx.slowmo > 0.0 {
        fx.slowmo = (fx.slowmo - dt).max(0.0);
    }
    let wanted = if fx.slowmo > 0.0 { SLOWMO_SCALE } else { 1.0 };
    if (virtual_time.relative_speed() - wanted).abs() > 0.001 {
        virtual_time.set_relative_speed(wanted);
    }

    if let AdPhase::Pending(waited) = ads.phase {
        // 0.3 的 `AdResult` 带 ticket，只有和本次请求一致才算数；适配层本身
        // 也会在 30s 后以 reason=timeout 结束，这里的 32s 只是双保险。
        if let Some(result) = host.ad_result() {
            if result.ticket == ads.ticket {
                match result.status.as_str() {
                    "ended" if result.is_ended => {
                        grant_slowmo(&mut fx);
                        ads.phase = AdPhase::Idle;
                        info!(
                            "pong: rewarded ad ended ticket={} mock={} reason={}, slow-mo granted",
                            result.ticket, result.mock, result.reason,
                        );
                    }
                    "skipped" => {
                        ads.phase = AdPhase::Error(1.0);
                        info!("pong: rewarded ad skipped ticket={}", result.ticket);
                    }
                    other => {
                        ads.phase = AdPhase::Error(1.2);
                        info!(
                            "pong: rewarded ad failed ticket={} status={} reason={} last_error={:?}",
                            result.ticket,
                            other,
                            result.reason,
                            host.ad_last_error(),
                        );
                    }
                }
                ads.ticket = 0;
            }
        } else if waited > 32.0 {
            ads.phase = AdPhase::Error(1.2);
            info!("pong: rewarded ad timed out waiting for AdResult");
        } else {
            ads.phase = AdPhase::Pending(waited + dt);
        }
    }

    if let AdPhase::Mock(left) = ads.phase {
        if left - dt <= 0.0 {
            grant_slowmo(&mut fx);
            ads.phase = AdPhase::Idle;
            info!("pong: mock ad finished, slow-mo granted");
        } else {
            ads.phase = AdPhase::Mock(left - dt);
        }
    }

    if let AdPhase::Error(left) = ads.phase {
        if left - dt <= 0.0 {
            ads.phase = AdPhase::Idle;
        } else {
            ads.phase = AdPhase::Error(left - dt);
        }
    }
}

fn grant_slowmo(fx: &mut Effects) {
    fx.slowmo = MAX_SLOWMO;
}

// ── 上色 ──────────────────────────────────────────────────────────────────────

fn paint_scene(
    look: Res<Look>,
    game: Res<Match>,
    ads: Res<Ads>,
    fx: Res<Effects>,
    real: Res<Time<Real>>,
    mut clear: ResMut<ClearColor>,
    mut sprites: Query<(&Themed, &mut Sprite, Option<&ScorePip>)>,
    mut progress: Query<&mut Transform, With<AdProgress>>,
) {
    let palette = look.palette();
    clear.0 = palette.bg;
    let t = real.elapsed_secs();

    for (themed, mut sprite, pip) in &mut sprites {
        let mut color = palette.color(themed.role);
        let mut alpha = themed.alpha;

        if let Some(pip) = pip {
            let filled = pip.index < game.score[pip.side.index()];
            color = if filled { palette.ball } else { palette.pip_off };
            alpha = if filled { 1.0 } else { 0.75 };
        }

        match themed.role {
            Role::Frame => {
                if let Phase::Over { winner, .. } = game.phase {
                    color = match winner {
                        Side::Player => palette.player,
                        Side::Ai => palette.ai,
                    };
                    alpha = 0.55 + 0.35 * (t * 7.0).sin().abs();
                } else if game.flash > 0.0 {
                    alpha = (alpha + game.flash * 0.6).min(1.0);
                }
            }
            Role::Chip => {
                if fx.slowmo > 0.0 {
                    color = palette.accent;
                    alpha = 0.65 + 0.35 * (t * 9.0).sin().abs();
                } else {
                    match ads.phase {
                        AdPhase::Idle => {
                            color = palette.chip;
                            alpha = 1.0;
                        }
                        AdPhase::Pending(_) => {
                            color = palette.chip;
                            alpha = 0.45 + 0.45 * (t * 12.0).sin().abs();
                        }
                        AdPhase::Mock(_) => {
                            color = palette.accent;
                            alpha = 0.8;
                        }
                        AdPhase::Error(_) => {
                            color = palette.warn;
                            alpha = 0.55 + 0.4 * (t * 20.0).sin().abs();
                        }
                    }
                }
            }
            Role::ChipIcon => {
                color = palette.chip_icon;
                alpha = 1.0;
            }
            Role::Record => {
                let is_record = game.rally > 0 && game.rally >= game.best_rally;
                alpha = if is_record {
                    0.65 + 0.35 * (t * 8.0).sin().abs()
                } else {
                    0.0
                };
            }
            _ => {}
        }

        if matches!(game.phase, Phase::Over { .. }) && matches!(themed.role, Role::Ball) {
            alpha = 0.0;
        }

        sprite.color = color.with_alpha(alpha.clamp(0.0, 1.0));
    }

    if let Ok(mut transform) = progress.single_mut() {
        let ratio = (fx.slowmo / MAX_SLOWMO).clamp(0.0, 1.0);
        transform.scale.x = ratio.max(0.001);
    }
}
