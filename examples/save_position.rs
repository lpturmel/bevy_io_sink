use bevy::prelude::*;
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use bevy_io_sink::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Default, Clone, Resource, Deserialize, Debug, Reflect)]
#[reflect(Resource)]
pub struct PlayerState {
    pos: Vec2,
}

#[derive(Component, Deref, DerefMut)]
struct PositionTimer(pub Timer);

#[derive(Component)]
struct Player;

fn main() {
    let file_sink: FileSinkPlugin<PlayerState> = FileSinkPlugin::new("test.json");

    App::new()
        .register_type::<PlayerState>()
        .add_plugins(file_sink)
        .add_plugins((DefaultPlugins, WorldInspectorPlugin::new()))
        .add_systems(Update, setup.run_if(resource_added::<PlayerState>))
        .add_systems(
            Update,
            player_movement.run_if(resource_exists::<PlayerState>),
        )
        .run();
}

fn setup(mut commands: Commands, state: Res<PlayerState>) {
    info!("Spawning player");
    commands.spawn((Camera2d, Transform::default()));

    commands.spawn((
        Sprite {
            custom_size: Some(Vec2::new(100.0, 100.0)),
            color: Color::srgb(0.0, 0.0, 1.0),
            ..Default::default()
        },
        Transform::from_translation(Vec3::new(state.pos.x, state.pos.y, 0.0)),
        PositionTimer(Timer::from_seconds(1.0, TimerMode::Repeating)),
        Player,
    ));
}

fn player_movement(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut query: Query<(&mut Transform, &mut PositionTimer), With<Player>>,
    sender: Res<IoSender<PlayerState>>,
    mut state: ResMut<PlayerState>,
    time: Res<Time>,
) {
    for (mut transform, mut timer) in query.iter_mut() {
        timer.tick(time.delta());

        let mut direction = Vec2::ZERO;
        if keyboard_input.pressed(KeyCode::KeyA) {
            direction.x -= 1.0;
        }
        if keyboard_input.pressed(KeyCode::KeyD) {
            direction.x += 1.0;
        }
        if keyboard_input.pressed(KeyCode::KeyS) {
            direction.y -= 1.0;
        }
        if keyboard_input.pressed(KeyCode::KeyW) {
            direction.y += 1.0;
        }
        if direction.length_squared() > 0.0 {
            let direction = direction.normalize() * 10.;
            transform.translation.x += direction.x;
            transform.translation.y += direction.y;

            state.pos += direction;
        }

        if timer.just_finished() {
            let _ = sender.try_send(state.clone());
        }
    }
}
