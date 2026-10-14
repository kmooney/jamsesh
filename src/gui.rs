//! Bevy window: two keyboards (you and the robot) with a tracker-style roll scrolling
//! downward beneath each, and buttons for tempo, robot octave, temperature and click.
//!
//! Everything dynamic is drawn from pooled sprites/texts re-laid out every frame from the
//! shared jam state, so the jam thread never waits on rendering.

use std::sync::Arc;
use std::sync::mpsc::Sender;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::sprite::Anchor;
use bevy::window::WindowResolution;

use crate::decide::Part;
use crate::jam::Shared;
use crate::viz::{Command, Who};

const WIDTH: f32 = 1440.0;
const HEIGHT: f32 = 900.0;
const HEADER: f32 = 64.0;
const LANE_MARGIN: f32 = 16.0;
const LANE_GAP: f32 = 24.0;
const TITLE_H: f32 = 26.0;
const WHITE_H: f32 = 96.0;
const BLACK_H: f32 = 60.0;
/// Keyboard range: C1..C7.
const LOW: u8 = 24;
const HIGH: u8 = 96;
const PX_PER_BEAT: f32 = 26.0;

const BG: Color = Color::srgb(0.035, 0.04, 0.07);
const ROLL_BG: Color = Color::srgb(0.06, 0.07, 0.11);
const ROLL_BG_ALT: Color = Color::srgb(0.085, 0.095, 0.145);
const BEAT_LINE: Color = Color::srgb(0.13, 0.15, 0.22);
const BAR_LINE: Color = Color::srgb(0.35, 0.40, 0.58);
const NOW_LINE: Color = Color::srgb(0.95, 0.25, 0.25);
const HUMAN: Color = Color::srgb(0.25, 0.80, 0.95);
const ROBOT: Color = Color::srgb(1.0, 0.62, 0.20);
const DIM_TEXT: Color = Color::srgb(0.55, 0.60, 0.75);
const BUTTON: Color = Color::srgb(0.14, 0.16, 0.24);
const BUTTON_HOVER: Color = Color::srgb(0.22, 0.25, 0.37);
const BUTTON_PRESS: Color = Color::srgb(0.35, 0.55, 0.35);

#[derive(Resource)]
pub struct Link {
    pub shared: Arc<Shared>,
    tx: Sender<Command>,
    beats_per_bar: u32,
}

/// Where one lane (keyboard + roll) sits on screen, in world coordinates.
#[derive(Clone, Copy)]
struct Lane {
    who: Who,
    left: f32,
    width: f32,
}

impl Lane {
    fn all() -> [Lane; 2] {
        let width = (WIDTH - 2.0 * LANE_MARGIN - LANE_GAP) / 2.0;
        let left = -WIDTH / 2.0 + LANE_MARGIN;
        [
            Lane { who: Who::Human, left, width },
            Lane { who: Who::Robot, left: left + width + LANE_GAP, width },
        ]
    }

    fn white_w(&self) -> f32 {
        self.width / white_count() as f32
    }

    /// Center x and width of a key column.
    fn key_x(&self, note: u8) -> (f32, f32) {
        let w = self.white_w();
        let whites_before = (LOW..note).filter(|n| !is_black(*n)).count() as f32;
        if is_black(note) {
            (self.left + whites_before * w, w * 0.62)
        } else {
            (self.left + (whites_before + 0.5) * w, w)
        }
    }

    fn color(&self) -> Color {
        match self.who {
            Who::Human => HUMAN,
            Who::Robot => ROBOT,
        }
    }
}

fn is_black(note: u8) -> bool {
    matches!(note % 12, 1 | 3 | 6 | 8 | 10)
}

fn white_count() -> usize {
    (LOW..=HIGH).filter(|n| !is_black(*n)).count()
}

// Vertical layout (y up, origin at window center).
fn keys_top() -> f32 {
    HEIGHT / 2.0 - HEADER - TITLE_H
}
fn now_y() -> f32 {
    keys_top() - WHITE_H - 6.0
}
fn roll_bottom() -> f32 {
    -HEIGHT / 2.0 + 8.0
}

#[derive(Component)]
struct PianoKey {
    who: Who,
    note: u8,
}

#[derive(Component)]
struct KeyDot {
    who: Who,
    note: u8,
}

#[derive(Component)]
struct PoolRect;

#[derive(Component)]
struct PoolText;

#[derive(Component)]
struct Action(Command);

/// Marks a part button, to outline the active one.
#[derive(Component)]
struct PartButton(Part);

#[derive(Component)]
enum Readout {
    Bpm,
    Octave,
    Temperature,
    Click,
    Info,
}

pub fn run(shared: Arc<Shared>, tx: Sender<Command>, beats_per_bar: u32) {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "jamsesh".into(),
                resolution: WindowResolution::new(WIDTH as u32, HEIGHT as u32),
                resizable: false,
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(BG))
        .insert_resource(Link { shared, tx, beats_per_bar })
        .add_plugins(crate::brain::plugin)
        .add_systems(Startup, (setup_keyboards, setup_header))
        .add_systems(
            Update,
            (buttons, shortcuts, update_keys, update_roll, update_header, update_part_buttons, self_screenshot),
        )
        .run();
}

fn setup_keyboards(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<ColorMaterial>>) {
    commands.spawn(Camera2d);
    let dot_mesh = meshes.add(Circle::new(4.0));
    let dot_material = materials.add(Color::srgb(0.95, 0.1, 0.1));
    let top = keys_top();

    for lane in Lane::all() {
        let title = match lane.who {
            Who::Human => "YOU",
            Who::Robot => "ROBOT",
        };
        commands.spawn((
            Text2d::new(title),
            TextFont { font_size: FontSize::Px(16.0), ..default() },
            TextColor(lane.color()),
            Anchor::BOTTOM_LEFT,
            Transform::from_xyz(lane.left, top + 5.0, 5.0),
        ));
        // Roll backdrop.
        let roll_h = now_y() - roll_bottom();
        commands.spawn((
            Sprite::from_color(ROLL_BG, Vec2::new(lane.width, roll_h)),
            Transform::from_xyz(lane.left + lane.width / 2.0, roll_bottom() + roll_h / 2.0, -2.0),
        ));

        for note in LOW..=HIGH {
            let (x, w) = lane.key_x(note);
            let (h, z, color) = if is_black(note) {
                (BLACK_H, 1.0, Color::srgb(0.08, 0.08, 0.1))
            } else {
                (WHITE_H, 0.0, Color::srgb(0.93, 0.93, 0.9))
            };
            commands.spawn((
                Sprite::from_color(color, Vec2::new(w - 1.5, h)),
                Transform::from_xyz(x, top - h / 2.0, z),
                PianoKey { who: lane.who, note },
            ));
            commands.spawn((
                Mesh2d(dot_mesh.clone()),
                MeshMaterial2d(dot_material.clone()),
                Transform::from_xyz(x, top - h + 11.0, 2.0),
                Visibility::Hidden,
                KeyDot { who: lane.who, note },
            ));
            // C labels under each octave.
            if note % 12 == 0 {
                commands.spawn((
                    Text2d::new(format!("C{}", note as i32 / 12 - 1)),
                    TextFont { font_size: FontSize::Px(10.0), ..default() },
                    TextColor(Color::srgb(0.35, 0.35, 0.4)),
                    Transform::from_xyz(x, top - WHITE_H + 24.0, 1.5),
                ));
            }
        }
    }
}

fn setup_header(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let brain = crate::brain::brain_icon(&mut images);
    let label = |text: &str| {
        (
            Text::new(text),
            TextFont { font_size: FontSize::Px(18.0), ..default() },
            TextColor(Color::srgb(0.9, 0.9, 0.95)),
            TextLayout::default().with_no_wrap(),
        )
    };
    let button = |text: &str, cmd: Command| {
        (
            Button,
            Action(cmd),
            Node {
                width: px(38),
                height: px(34),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BorderColor::all(Color::srgb(0.3, 0.33, 0.45)),
            BackgroundColor(BUTTON),
            children![(Text::new(text), TextFont { font_size: FontSize::Px(20.0), ..default() })],
        )
    };
    let part_button = |part: Part| {
        (
            Button,
            Action(Command::Part(part)),
            PartButton(part),
            Node {
                height: px(34),
                padding: UiRect::horizontal(px(10)),
                align_items: AlignItems::Center,
                border: UiRect::all(px(2)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BorderColor::all(Color::srgb(0.3, 0.33, 0.45)),
            BackgroundColor(BUTTON),
            children![(
                Text::new(part.name()),
                TextFont { font_size: FontSize::Px(16.0), ..default() },
                TextLayout::default().with_no_wrap(),
            )],
        )
    };
    let group = || Node {
        align_items: AlignItems::Center,
        column_gap: px(6),
        margin: UiRect::right(px(28)),
        ..default()
    };

    commands.spawn((
        Node {
            width: percent(100),
            height: px(HEADER),
            padding: UiRect::horizontal(px(16)),
            align_items: AlignItems::Center,
            overflow: Overflow::clip(),
            ..default()
        },
        BackgroundColor(Color::srgb(0.07, 0.08, 0.12)),
        children![
            (group(), children![
                (label("120 BPM"), Readout::Bpm, Node { width: px(96), ..default() }),
                button("-", Command::Bpm(-2.0)),
                button("+", Command::Bpm(2.0)),
            ]),
            (group(), children![
                (label("robot octave +0"), Readout::Octave, Node { width: px(186), ..default() }),
                button("-", Command::Octave(-1)),
                button("+", Command::Octave(1)),
            ]),
            (group(), children![
                (label("temp 0.5"), Readout::Temperature, Node { width: px(86), ..default() }),
                button("-", Command::Temperature(-0.1)),
                button("+", Command::Temperature(0.1)),
            ]),
            (group(), children![
                (
                    Button,
                    Action(Command::ToggleClick),
                    Node {
                        height: px(34),
                        min_width: px(116),
                        justify_content: JustifyContent::Center,
                        padding: UiRect::horizontal(px(12)),
                        align_items: AlignItems::Center,
                        border: UiRect::all(px(1)),
                        border_radius: BorderRadius::all(px(6)),
                        ..default()
                    },
                    BorderColor::all(Color::srgb(0.3, 0.33, 0.45)),
                    BackgroundColor(BUTTON),
                    children![(label("click off"), Readout::Click)],
                ),
            ]),
            (group(), children![
                (label("robot"), Node { margin: UiRect::right(px(4)), ..default() }),
                part_button(Part::Ambient),
                part_button(Part::Bass),
                part_button(Part::Notes),
                part_button(Part::Chords),
                part_button(Part::Pad),
            ]),
            (
                Button,
                crate::brain::BrainButton,
                Node {
                    height: px(36),
                    padding: UiRect::horizontal(px(8)),
                    column_gap: px(6),
                    margin: UiRect::right(px(20)),
                    align_items: AlignItems::Center,
                    border: UiRect::all(px(1)),
                    border_radius: BorderRadius::all(px(6)),
                    ..default()
                },
                BorderColor::all(Color::srgb(0.3, 0.33, 0.45)),
                BackgroundColor(BUTTON),
                children![
                    (ImageNode::new(brain), Node { width: px(26), height: px(26), ..default() }),
                    (Text::new("Jev"), TextFont { font_size: FontSize::Px(16.0), ..default() }),
                ],
            ),
            (
                Text::new(""),
                TextFont { font_size: FontSize::Px(15.0), ..default() },
                TextColor(DIM_TEXT),
                TextLayout::default().with_no_wrap(),
                Readout::Info,
            ),
        ],
    ));
}

fn buttons(
    link: Res<Link>,
    mut query: Query<(&Interaction, &Action, &mut BackgroundColor), Changed<Interaction>>,
) {
    for (interaction, action, mut bg) in &mut query {
        *bg = match interaction {
            Interaction::Pressed => {
                let _ = link.tx.send(action.0);
                BUTTON_PRESS.into()
            }
            Interaction::Hovered => BUTTON_HOVER.into(),
            Interaction::None => BUTTON.into(),
        };
    }
}

fn shortcuts(link: Res<Link>, keys: Res<ButtonInput<KeyCode>>, mut exit: MessageWriter<AppExit>) {
    let send = |c| {
        let _ = link.tx.send(c);
    };
    if keys.any_just_pressed([KeyCode::Equal, KeyCode::NumpadAdd, KeyCode::ArrowUp]) {
        send(Command::Bpm(2.0));
    }
    if keys.any_just_pressed([KeyCode::Minus, KeyCode::NumpadSubtract, KeyCode::ArrowDown]) {
        send(Command::Bpm(-2.0));
    }
    if keys.just_pressed(KeyCode::BracketRight) {
        send(Command::Octave(1));
    }
    if keys.just_pressed(KeyCode::BracketLeft) {
        send(Command::Octave(-1));
    }
    if keys.just_pressed(KeyCode::Period) {
        send(Command::Temperature(0.1));
    }
    if keys.just_pressed(KeyCode::Comma) {
        send(Command::Temperature(-0.1));
    }
    for (k, part) in [(KeyCode::Digit1, Part::Ambient), (KeyCode::Digit2, Part::Bass), (KeyCode::Digit3, Part::Notes), (KeyCode::Digit4, Part::Chords), (KeyCode::Digit5, Part::Pad)] {
        if keys.just_pressed(k) {
            send(Command::Part(part));
        }
    }
    if keys.just_pressed(KeyCode::KeyC) {
        send(Command::ToggleClick);
    }
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
}

fn update_keys(
    link: Res<Link>,
    mut keys: Query<(&PianoKey, &mut Sprite)>,
    mut dots: Query<(&KeyDot, &mut Visibility)>,
) {
    let viz = link.shared.viz();
    let mut down = [[false; 128]; 2];
    for n in viz.notes.iter().filter(|n| n.end.is_none()) {
        down[n.who as usize][n.note as usize] = true;
    }
    drop(viz);
    for (key, mut sprite) in &mut keys {
        let on = down[key.who as usize][key.note as usize];
        let lane = if key.who == Who::Human { HUMAN } else { ROBOT };
        sprite.color = match (on, is_black(key.note)) {
            (true, false) => Color::srgb(0.93, 0.93, 0.9).mix(&lane, 0.45),
            (true, true) => Color::srgb(0.08, 0.08, 0.1).mix(&lane, 0.55),
            (false, false) => Color::srgb(0.93, 0.93, 0.9),
            (false, true) => Color::srgb(0.08, 0.08, 0.1),
        };
    }
    for (dot, mut vis) in &mut dots {
        *vis = if down[dot.who as usize][dot.note as usize] { Visibility::Visible } else { Visibility::Hidden };
    }
}

struct RectSpec {
    center: Vec2,
    size: Vec2,
    color: Color,
    z: f32,
}

struct TextSpec {
    pos: Vec2,
    text: String,
    color: Color,
    size: f32,
}

fn update_roll(
    mut commands: Commands,
    link: Res<Link>,
    mut rects: Query<(&mut Sprite, &mut Transform, &mut Visibility), (With<PoolRect>, Without<PoolText>)>,
    mut texts: Query<(&mut Text, &mut TextColor, &mut TextFont, &mut Node, &mut Visibility), (With<PoolText>, Without<PoolRect>)>,
) {
    let now = link.shared.now();
    let bpb = link.beats_per_bar as f64;
    let (top, bottom) = (now_y(), roll_bottom());
    let y_of = |beat: f64| top - ((now - beat) as f32) * PX_PER_BEAT;
    let oldest = now - ((top - bottom) / PX_PER_BEAT) as f64;

    let mut r: Vec<RectSpec> = Vec::new();
    let mut t: Vec<TextSpec> = Vec::new();
    let lanes = Lane::all();

    // Bars: alternating shading, beat lines, bar lines and numbers.
    let first_beat = oldest.floor() as i64;
    for beat in first_beat..=now.floor() as i64 {
        let y = y_of(beat as f64);
        let bar_line = beat.rem_euclid(bpb as i64) == 0;
        let bar = beat.div_euclid(bpb as i64);
        for lane in &lanes {
            let cx = lane.left + lane.width / 2.0;
            if bar_line && bar.rem_euclid(2) == 1 {
                // Shade every other bar, clipped to the roll.
                let y1 = y.max(bottom);
                let y2 = y_of(beat as f64 + bpb).min(top);
                if y2 > y1 {
                    r.push(RectSpec { center: Vec2::new(cx, (y1 + y2) / 2.0), size: Vec2::new(lane.width, y2 - y1), color: ROLL_BG_ALT, z: -1.5 });
                }
            }
            if y >= bottom {
                let (color, h) = if bar_line { (BAR_LINE, 2.0) } else { (BEAT_LINE, 1.0) };
                r.push(RectSpec { center: Vec2::new(cx, y), size: Vec2::new(lane.width, h), color, z: -1.0 });
                if bar_line && bar >= 0 {
                    t.push(TextSpec { pos: Vec2::new(lane.left + lane.width - 4.0, y + 2.0), text: format!("{bar:03}"), color: BAR_LINE, size: 11.0 });
                }
            }
        }
    }
    // The "now" line.
    for lane in &lanes {
        r.push(RectSpec { center: Vec2::new(lane.left + lane.width / 2.0, top), size: Vec2::new(lane.width, 2.0), color: NOW_LINE, z: 3.0 });
    }

    // Notes and labels.
    let viz = link.shared.viz();
    for n in &viz.notes {
        let end = n.end.unwrap_or(now);
        if end < oldest || n.note < LOW || n.note > HIGH {
            continue;
        }
        let lane = lanes[n.who as usize];
        let (x, w) = lane.key_x(n.note);
        let y1 = y_of(n.start).max(bottom);
        let y2 = y_of(end).min(top);
        let held = n.end.is_none();
        let color = if held { lane.color().lighter(0.15) } else { lane.color().darker(0.12) };
        r.push(RectSpec { center: Vec2::new(x, (y1 + y2) / 2.0), size: Vec2::new((w * 0.8).max(4.0), (y2 - y1).max(2.0)), color, z: -0.5 });
    }
    for l in &viz.labels {
        let y = y_of(l.beat);
        if y < bottom || y > top - 12.0 {
            continue;
        }
        let lane = lanes[l.who as usize];
        t.push(TextSpec { pos: Vec2::new(lane.left + 4.0, y + 2.0), text: l.text.clone(), color: lane.color(), size: 13.0 });
    }
    drop(viz);

    // Hand the specs to pooled entities, growing the pools as needed.
    let mut rect_iter = rects.iter_mut();
    for spec in &r {
        match rect_iter.next() {
            Some((mut sprite, mut tf, mut vis)) => {
                sprite.color = spec.color;
                sprite.custom_size = Some(spec.size);
                tf.translation = spec.center.extend(spec.z);
                *vis = Visibility::Visible;
            }
            None => {
                commands.spawn((
                    Sprite::from_color(spec.color, spec.size),
                    Transform::from_translation(spec.center.extend(spec.z)),
                    PoolRect,
                ));
            }
        }
    }
    for (_, _, mut vis) in rect_iter {
        *vis = Visibility::Hidden;
    }

    // Labels are UI text so they always draw over the notes. Positions are converted from
    // world space (y up, centered) to UI space (y down, from the top-left).
    let mut text_iter = texts.iter_mut();
    for spec in &t {
        let top = HEIGHT / 2.0 - spec.pos.y - spec.size * 1.3 - 2.0;
        // Bar numbers hug the right edge; labels the left.
        let (left, right) = if spec.size < 12.0 {
            (Val::Auto, px(WIDTH / 2.0 - spec.pos.x))
        } else {
            (px(spec.pos.x + WIDTH / 2.0), Val::Auto)
        };
        match text_iter.next() {
            Some((mut text, mut color, mut font, mut node, mut vis)) => {
                if text.0 != spec.text {
                    text.0.clone_from(&spec.text);
                }
                color.0 = spec.color;
                font.font_size = FontSize::Px(spec.size);
                node.top = px(top);
                node.left = left;
                node.right = right;
                *vis = Visibility::Visible;
            }
            None => {
                commands.spawn((
                    Text::new(spec.text.clone()),
                    TextColor(spec.color),
                    TextFont { font_size: FontSize::Px(spec.size), ..default() },
                    TextLayout::default().with_no_wrap(),
                    Node {
                        position_type: PositionType::Absolute,
                        top: px(top),
                        left,
                        right,
                        padding: UiRect::horizontal(px(3)),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.03, 0.035, 0.06, 0.75)),
                    PoolText,
                ));
            }
        }
    }
    for (_, _, _, _, mut vis) in text_iter {
        *vis = Visibility::Hidden;
    }
}

/// Active part: solid orange outline; one waiting for the next bar: dim outline.
fn update_part_buttons(link: Res<Link>, mut buttons: Query<(&PartButton, &mut BorderColor)>) {
    let viz = link.shared.viz();
    for (b, mut border) in &mut buttons {
        let color = if viz.pending_part == Some(b.0) {
            ROBOT.with_alpha(0.45)
        } else if viz.part == Some(b.0) && viz.pending_part.is_none() {
            ROBOT
        } else {
            Color::srgb(0.3, 0.33, 0.45)
        };
        *border = BorderColor::all(color);
    }
}

/// For testing: JAMSESH_SCREENSHOT=<path> presses BPM+ three times, octave+ and temp+ once and "chords" (the same
/// commands the buttons send), saves one frame after 12 s (or JAMSESH_SCREENSHOT_AT seconds), then quits.
fn self_screenshot(
    mut commands: Commands,
    link: Res<Link>,
    time: Res<Time>,
    mut done: Local<bool>,
    mut pressed: Local<bool>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(path) = std::env::var("JAMSESH_SCREENSHOT") else { return };
    let t = time.elapsed_secs();
    let at: f32 = std::env::var("JAMSESH_SCREENSHOT_AT").ok().and_then(|s| s.parse().ok()).unwrap_or(12.0);
    if t > 4.0 && !*pressed {
        *pressed = true;
        for c in [Command::Bpm(2.0), Command::Bpm(2.0), Command::Bpm(2.0), Command::Octave(1), Command::Temperature(0.1), Command::Part(Part::Chords)] {
            let _ = link.tx.send(c);
        }
    }
    if t > at && !*done {
        *done = true;
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
    }
    if t > at + 2.0 {
        exit.write(AppExit::Success);
    }
}

fn update_header(link: Res<Link>, mut readouts: Query<(&Readout, &mut Text)>) {
    let bpm = link.shared.bpm();
    let viz = link.shared.viz();
    for (readout, mut text) in &mut readouts {
        let s = match readout {
            Readout::Bpm => format!("{bpm:.0} BPM"),
            Readout::Octave => format!("robot octave {:+}", viz.octave),
            Readout::Temperature => format!("temp {:.1}", viz.temperature),
            Readout::Click => format!("click {}", if viz.click { "on" } else { "off" }),
            // The default font has no middle dot.
            Readout::Info => format!("{}  |  {}", viz.key, viz.status).replace('·', "|"),
        };
        if text.0 != s {
            text.0 = s;
        }
    }
}
