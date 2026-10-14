//! The brain panel: an overlay in the jam window with charts of Jev's confidence over time,
//! then each request sent to Jev and its response, laid out as indented fields rather than
//! JSON. Opened with the brain button (or J); the jam keeps playing underneath.

use bevy::asset::RenderAssetUsages;
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::jev_view::{self, Kind};

const PANEL_W: f32 = 660.0;
const KEY: Color = Color::srgb(0.55, 0.62, 0.8);
const VALUE: Color = Color::srgb(0.92, 0.92, 0.95);
const SECTION: Color = Color::srgb(0.25, 0.8, 0.95);
const ACCENT: Color = Color::srgb(1.0, 0.62, 0.2);
const BAR_OFF: Color = Color::srgb(0.3, 0.33, 0.45);
const WARN: Color = Color::srgb(1.0, 0.35, 0.35);
const CHART_BG: Color = Color::srgb(0.06, 0.07, 0.11);
const GRID: Color = Color::srgb(0.16, 0.18, 0.26);
/// Chart plot size in logical pixels (drawn at twice that for sharp lines); shorter when
/// there are several charts, so the exchange below stays readable.
const CHART_W: f32 = 590.0;
fn chart_h(charts: usize) -> f32 {
    if charts <= 2 { 76.0 } else { 40.0 }
}
const LINE_COLORS: [Color; jev_view::PALETTE] = [
    Color::srgb(1.0, 0.62, 0.2),
    Color::srgb(0.25, 0.8, 0.95),
    Color::srgb(0.95, 0.45, 0.7),
    Color::srgb(0.45, 0.85, 0.45),
    Color::srgb(0.95, 0.87, 0.3),
    Color::srgb(0.65, 0.55, 1.0),
    Color::srgb(1.0, 0.38, 0.33),
    Color::srgb(0.3, 0.75, 0.65),
    Color::srgb(0.88, 0.88, 0.92),
    Color::srgb(0.8, 0.6, 0.4),
];

#[derive(Resource, Default)]
pub struct BrainPanel {
    open: bool,
    /// Exchange number being shown; None follows the latest.
    selected: Option<u64>,
    shown: Option<u64>,
    /// Jump back to the top after the user navigates.
    reset_scroll: bool,
}

#[derive(Component)]
pub struct BrainButton;

#[derive(Component)]
struct Panel;

#[derive(Component)]
struct PanelScroll;

#[derive(Component)]
struct PanelContent;

#[derive(Component)]
struct PanelNav;

#[derive(Component)]
struct ChartArea;

#[derive(Component, Clone, Copy)]
enum PanelAction {
    Older,
    Newer,
    Latest,
    Close,
}

pub fn plugin(app: &mut App) {
    app.init_resource::<BrainPanel>()
        .add_systems(Startup, spawn_panel)
        .add_systems(Update, (panel_buttons, panel_keys, update_panel, update_charts, panel_scroll));
}

/// A small pink brain, drawn procedurally (the default font has no emoji).
pub fn brain_icon(images: &mut Assets<Image>) -> Handle<Image> {
    const S: u32 = 32;
    let mut img = Image::new_fill(
        Extent3d { width: S, height: S, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0, 0, 0, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    let inside = |x: f32, y: f32| {
        let lobe = |cx: f32| ((x - cx) / 8.5).powi(2) + ((y - 15.5) / 11.0).powi(2) <= 1.0;
        let stem = ((x - 18.0) / 3.0).powi(2) + ((y - 27.0) / 3.5).powi(2) <= 1.0;
        lobe(10.5) || lobe(21.5) || stem
    };
    for y in 0..S {
        for x in 0..S {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            if !inside(fx, fy) {
                continue;
            }
            let edge = !inside(fx + 1.2, fy) || !inside(fx - 1.2, fy) || !inside(fx, fy + 1.2) || !inside(fx, fy - 1.2);
            let fold = ((fx * 0.75).sin() + (fy * 0.9).cos() * 0.8).abs() < 0.22 || (fx - 16.0).abs() < 0.8 && fy < 25.0;
            let color = if edge {
                Color::srgb(0.55, 0.2, 0.32)
            } else if fold {
                Color::srgb(0.78, 0.36, 0.5)
            } else {
                Color::srgb(0.98, 0.66, 0.76)
            };
            let _ = img.set_color_at(x, y, color);
        }
    }
    images.add(img)
}

fn spawn_panel(mut commands: Commands) {
    let nav_button = |text: &str, action: PanelAction| {
        (
            Button,
            action,
            Node {
                height: px(28),
                padding: UiRect::horizontal(px(10)),
                align_items: AlignItems::Center,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(5)),
                ..default()
            },
            BorderColor::all(BAR_OFF),
            BackgroundColor(Color::srgb(0.14, 0.16, 0.24)),
            children![(Text::new(text), TextFont { font_size: FontSize::Px(14.0), ..default() })],
        )
    };
    commands.spawn((
        Panel,
        Node {
            display: Display::None,
            position_type: PositionType::Absolute,
            right: px(12),
            top: px(72),
            bottom: px(12),
            width: px(PANEL_W),
            flex_direction: FlexDirection::Column,
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(8)),
            ..default()
        },
        BackgroundColor(Color::srgb(0.035, 0.04, 0.065)),
        BorderColor::all(ACCENT.with_alpha(0.6)),
        GlobalZIndex(100),
        children![
            (
                Node {
                    align_items: AlignItems::Center,
                    column_gap: px(6),
                    padding: UiRect::all(px(8)),
                    ..default()
                },
                children![
                    (Text::new("JEV"), TextFont { font_size: FontSize::Px(16.0), ..default() }, TextColor(ACCENT)),
                    (
                        Text::new(""),
                        PanelNav,
                        TextFont { font_size: FontSize::Px(13.0), ..default() },
                        TextColor(KEY),
                        Node { flex_grow: 1.0, margin: UiRect::left(px(6)), ..default() },
                    ),
                    nav_button("< older", PanelAction::Older),
                    nav_button("newer >", PanelAction::Newer),
                    nav_button("latest", PanelAction::Latest),
                    nav_button("x", PanelAction::Close),
                ],
            ),
            (
                ChartArea,
                Node {
                    flex_direction: FlexDirection::Column,
                    flex_shrink: 0.0,
                    row_gap: px(6),
                    padding: UiRect::new(px(12), px(12), px(0), px(8)),
                    border: UiRect::bottom(px(1)),
                    ..default()
                },
                BorderColor::all(GRID),
            ),
            (
                PanelScroll,
                Node {
                    flex_grow: 1.0,
                    min_height: px(0),
                    overflow: Overflow::scroll_y(),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::new(px(12), px(12), px(4), px(12)),
                    ..default()
                },
                ScrollPosition::default(),
                children![(PanelContent, Node { flex_direction: FlexDirection::Column, row_gap: px(2), ..default() })],
            ),
        ],
    ));
}

fn toggle(panel: &mut BrainPanel) {
    panel.open = !panel.open;
    panel.shown = None;
}

fn panel_buttons(
    mut panel: ResMut<BrainPanel>,
    mut query: Query<
        (&Interaction, Option<&BrainButton>, Option<&PanelAction>, &mut BackgroundColor),
        (Changed<Interaction>, Or<(With<BrainButton>, With<PanelAction>)>),
    >,
    link: Res<crate::gui::Link>,
) {
    for (interaction, brain, action, mut bg) in &mut query {
        *bg = match interaction {
            Interaction::Pressed => {
                if brain.is_some() {
                    toggle(&mut panel);
                }
                if let Some(action) = action {
                    let (first, last) = {
                        let viz = link.shared.viz();
                        (viz.exchange_count - viz.exchanges.len() as u64, viz.exchange_count.saturating_sub(1))
                    };
                    let current = panel.selected.unwrap_or(last);
                    match action {
                        PanelAction::Older => panel.selected = Some(current.saturating_sub(1).max(first)),
                        PanelAction::Newer => {
                            panel.selected = if current + 1 >= last { None } else { Some(current + 1) };
                        }
                        PanelAction::Latest => panel.selected = None,
                        PanelAction::Close => panel.open = false,
                    }
                    panel.reset_scroll = true;
                }
                Color::srgb(0.35, 0.55, 0.35).into()
            }
            Interaction::Hovered => Color::srgb(0.22, 0.25, 0.37).into(),
            Interaction::None => Color::srgb(0.14, 0.16, 0.24).into(),
        };
    }
}

fn panel_keys(keys: Res<ButtonInput<KeyCode>>, mut panel: ResMut<BrainPanel>, time: Res<Time>, mut test_opened: Local<bool>) {
    if keys.just_pressed(KeyCode::KeyJ) {
        toggle(&mut panel);
    }
    // The screenshot test (JAMSESH_SCREENSHOT) also shows the panel.
    if !*test_opened && time.elapsed_secs() > 6.0 && std::env::var_os("JAMSESH_SCREENSHOT").is_some() {
        *test_opened = true;
        panel.open = true;
        panel.shown = None;
    }
}

fn update_panel(
    mut commands: Commands,
    link: Res<crate::gui::Link>,
    mut panel: ResMut<BrainPanel>,
    mut panel_node: Query<&mut Node, With<Panel>>,
    content: Query<Entity, With<PanelContent>>,
    mut nav: Query<&mut Text, With<PanelNav>>,
    mut scroll: Query<&mut ScrollPosition, With<PanelScroll>>,
) {
    let display = if panel.open { Display::Flex } else { Display::None };
    for mut node in &mut panel_node {
        if node.display != display {
            node.display = display;
        }
    }
    if !panel.open {
        return;
    }
    let viz = link.shared.viz();
    let count = viz.exchange_count;
    let len = viz.exchanges.len() as u64;
    let nav_text = if len == 0 {
        "waiting for the first request...".to_string()
    } else {
        let first = count - len;
        let id = panel.selected.map(|s| s.clamp(first, count - 1)).unwrap_or(count - 1);
        let follow = if panel.selected.is_none() { "following latest" } else { "paused" };
        format!("{} of {}  |  {follow}", id - first + 1, len)
    };
    for mut t in &mut nav {
        if t.0 != nav_text {
            t.0.clone_from(&nav_text);
        }
    }
    if len == 0 {
        return;
    }
    let first = count - len;
    let id = panel.selected.map(|s| s.clamp(first, count - 1)).unwrap_or(count - 1);
    if panel.shown == Some(id) {
        return;
    }
    let lines = jev_view::lines(&viz.exchanges[(id - first) as usize]);
    drop(viz);
    panel.shown = Some(id);

    let Ok(content) = content.single() else { return };
    commands.entity(content).despawn_children();
    commands.entity(content).with_children(|c| {
        for l in lines {
            spawn_line(c, l);
        }
    });
    if std::mem::take(&mut panel.reset_scroll) {
        for mut s in &mut scroll {
            s.0 = Vec2::ZERO;
        }
    }
}

fn text(s: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (Text::new(s), TextFont { font_size: FontSize::Px(size), ..default() }, TextColor(color))
}

/// Single-line text that never wraps (labels, percentages).
fn label(s: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (text(s, size, color), TextLayout::default().with_no_wrap())
}

fn spawn_line(c: &mut ChildSpawnerCommands, l: jev_view::Line) {
    let indent = px(l.indent as f32 * 18.0);
    let row = |top: f32| Node {
        padding: UiRect::left(indent),
        margin: UiRect::top(px(top)),
        column_gap: px(8),
        align_items: AlignItems::Center,
        ..default()
    };
    match l.kind {
        Kind::Title => {
            c.spawn(row(0.0)).with_children(|r| {
                r.spawn(label(l.key, 18.0, ACCENT));
                r.spawn(label(l.value, 13.0, KEY));
            });
        }
        Kind::Section => {
            c.spawn(row(12.0)).with_children(|r| {
                r.spawn(text(l.key, 13.0, SECTION));
            });
        }
        Kind::Field => {
            c.spawn(row(0.0)).with_children(|r| {
                r.spawn((label(l.key, 14.0, KEY), Node { flex_shrink: 0.0, ..default() }));
                if !l.value.is_empty() {
                    r.spawn((text(l.value, 14.0, VALUE), Node { flex_shrink: 1.0, ..default() }));
                }
            });
        }
        Kind::Text => {
            c.spawn(row(0.0)).with_children(|r| {
                r.spawn(text(l.key, 14.0, Color::srgb(0.78, 0.8, 0.86)));
            });
        }
        Kind::Warn => {
            c.spawn(row(4.0)).with_children(|r| {
                r.spawn(text(format!("{}: {}", l.key, l.value), 14.0, WARN));
            });
        }
        // Over time these are in the charts; here just the numbers for this exchange.
        Kind::Prob(p, chosen) => {
            c.spawn(row(0.0)).with_children(|r| {
                // min_width, so long labels (score levels) push the number along instead of overlapping it.
                r.spawn((label(l.key, 13.0, if chosen { VALUE } else { KEY }), Node { min_width: px(210), flex_shrink: 0.0, ..default() }));
                r.spawn(label(format!("{:.0}%", p * 100.0), 13.0, if chosen { ACCENT } else { KEY }));
            });
        }
    }
}

/// Redraw the confidence charts when a new answer arrives (or the part changes).
fn update_charts(
    mut commands: Commands,
    link: Res<crate::gui::Link>,
    panel: Res<BrainPanel>,
    mut images: ResMut<Assets<Image>>,
    area: Query<Entity, With<ChartArea>>,
    mut drawn: Local<Option<u64>>,
) {
    if !panel.open {
        *drawn = None;
        return;
    }
    let mut viz = link.shared.viz();
    if *drawn == Some(viz.confidence.version) {
        return;
    }
    *drawn = Some(viz.confidence.version);
    let charts: Vec<_> = viz
        .confidence
        .charts
        .iter_mut()
        .map(|c| {
            let (series, hidden) = c.series();
            (c.name.clone(), c.points.len(), series, hidden)
        })
        .collect();
    drop(viz);

    let Ok(area) = area.single() else { return };
    commands.entity(area).despawn_children();
    commands.entity(area).with_children(|a| {
        if charts.is_empty() {
            a.spawn(text("Jev's confidence over time appears here once it answers (not in mock mode).", 13.0, KEY));
            return;
        }
        let h = chart_h(charts.len());
        for (name, points, series, hidden) in charts {
            let image = images.add(chart_image(&series, h));
            a.spawn(Node { column_gap: px(8), align_items: AlignItems::Baseline, ..default() }).with_children(|r| {
                r.spawn(label(name.to_uppercase(), 13.0, SECTION));
                r.spawn(label(format!("confidence over the last {points} answers"), 12.0, KEY));
            });
            a.spawn(Node { column_gap: px(6), ..default() }).with_children(|r| {
                r.spawn(Node {
                    width: px(30),
                    height: px(h),
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::End,
                    ..default()
                })
                .with_children(|axis| {
                    for t in ["100%", "50%", "0%"] {
                        axis.spawn(label(t, 10.0, KEY));
                    }
                });
                r.spawn((ImageNode::new(image), Node { width: px(CHART_W), height: px(h), ..default() }));
            });
            a.spawn(Node { flex_wrap: FlexWrap::Wrap, column_gap: px(14), row_gap: px(2), padding: UiRect::left(px(36)), ..default() })
                .with_children(|legend| {
                    for s in &series {
                        let latest = s.values.last().copied().flatten().map(|p| format!(" {:.0}%", p * 100.0)).unwrap_or_default();
                        legend.spawn(Node { column_gap: px(5), align_items: AlignItems::Center, ..default() }).with_children(|item| {
                            item.spawn((Node { width: px(10), height: px(10), ..default() }, BackgroundColor(LINE_COLORS[s.color])));
                            item.spawn(label(format!("{}{latest}", s.label), 12.0, VALUE));
                        });
                    }
                    if hidden > 0 {
                        legend.spawn(label(format!("+{hidden} less likely"), 12.0, KEY));
                    }
                });
        }
    });
}

/// One chart: gridlines at 0/50/100%, a line per option, newest point on the right once full.
fn chart_image(series: &[jev_view::Series], height: f32) -> Image {
    let (w, h) = ((CHART_W * 2.0) as u32, (height * 2.0) as u32);
    let to_rgba = |c: Color| {
        let c = c.to_srgba();
        [(c.red * 255.0) as u8, (c.green * 255.0) as u8, (c.blue * 255.0) as u8, 255]
    };
    let mut img = Image::new_fill(
        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &to_rgba(CHART_BG),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    let pad = 4.0;
    let y_of = |p: f64| pad + (1.0 - p as f32) * (h as f32 - 2.0 * pad);
    let x_of = |i: usize| pad + i as f32 * (w as f32 - 2.0 * pad) / (jev_view::POINTS - 1) as f32;
    let dot = |img: &mut Image, x: f32, y: f32, r: f32, color: Color| {
        let r2 = r * r;
        for py in (y - r).floor() as i32..=(y + r).ceil() as i32 {
            for px in (x - r).floor() as i32..=(x + r).ceil() as i32 {
                let (dx, dy) = (px as f32 - x, py as f32 - y);
                if dx * dx + dy * dy <= r2 && px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                    let _ = img.set_color_at(px as u32, py as u32, color);
                }
            }
        }
    };
    for p in [0.0, 0.5, 1.0] {
        let y = y_of(p).round() as u32;
        for x in 0..w {
            let _ = img.set_color_at(x, y.min(h - 1), GRID);
        }
    }
    // Currently least likely first, so the leading option's line ends up on top.
    let mut order: Vec<&jev_view::Series> = series.iter().collect();
    order.sort_by(|a, b| a.values.last().copied().flatten().unwrap_or(0.0).total_cmp(&b.values.last().copied().flatten().unwrap_or(0.0)));
    for s in order {
        let color = LINE_COLORS[s.color];
        let points: Vec<Option<(f32, f32)>> = s.values.iter().enumerate().map(|(i, v)| v.map(|p| (x_of(i), y_of(p)))).collect();
        for pair in points.windows(2) {
            if let [Some((x1, y1)), Some((x2, y2))] = pair {
                let steps = ((x2 - x1).hypot(y2 - y1) * 2.0).ceil() as usize;
                for k in 0..=steps {
                    let t = k as f32 / steps.max(1) as f32;
                    dot(&mut img, x1 + (x2 - x1) * t, y1 + (y2 - y1) * t, 2.0, color);
                }
            }
        }
        for &(x, y) in points.iter().flatten() {
            dot(&mut img, x, y, 3.2, color);
        }
    }
    img
}

fn panel_scroll(
    mut wheel: MessageReader<MouseWheel>,
    panel: Res<BrainPanel>,
    mut q: Query<(&mut ScrollPosition, &ComputedNode), With<PanelScroll>>,
) {
    for ev in wheel.read() {
        if !panel.open {
            continue;
        }
        let dy = match ev.unit {
            MouseScrollUnit::Line => -ev.y * 42.0,
            MouseScrollUnit::Pixel => -ev.y,
        };
        for (mut pos, node) in &mut q {
            let max = ((node.content_size() - node.size()) * node.inverse_scale_factor()).y.max(0.0);
            pos.0.y = (pos.0.y + dy).clamp(0.0, max);
        }
    }
}
