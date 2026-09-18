//! The brain panel: a scrollable overlay in the jam window showing each request sent to Jev
//! and its response, laid out as indented fields rather than JSON. Opened with the brain
//! button (or J); the jam keeps playing underneath.

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
        .add_systems(Update, (panel_buttons, panel_keys, update_panel, panel_scroll));
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
        Kind::Prob(p, chosen) => {
            let color = if chosen { ACCENT } else { BAR_OFF };
            c.spawn(row(0.0)).with_children(|r| {
                r.spawn((label(l.key, 13.0, if chosen { VALUE } else { KEY }), Node { width: px(210), flex_shrink: 0.0, ..default() }));
                r.spawn((
                    Node { width: px((p as f32 * 240.0).max(2.0)), height: px(9), flex_shrink: 0.0, ..default() },
                    BackgroundColor(color),
                ));
                r.spawn(label(format!("{:.0}%", p * 100.0), 13.0, if chosen { ACCENT } else { KEY }));
            });
        }
    }
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
