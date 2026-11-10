#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod game;

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;
use std::time::Duration;

use game::{Direction, Game, GameStore, MergedTile, MoveTransition};
use slint::{
    CloseRequestResponse, ComponentHandle, Image, LogicalSize, ModelRc, PhysicalPosition,
    Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode, VecModel,
};

slint::include_modules!();

const MOVE_ANIMATION_FINISH_DELAY: Duration = Duration::from_millis(100);
const MERGE_GROW_FINISH_DELAY: Duration = Duration::from_millis(60);
const MERGE_SHRINK_FINISH_DELAY: Duration = Duration::from_millis(120);
const MERGE_INTERRUPT_FINISH_DELAY: Duration = Duration::from_millis(30);
const BOARD_TILE_COUNT: usize = 16;
const RESET_WINDOW_WIDTH: f32 = 368.0;
const RESET_WINDOW_HEIGHT: f32 = 544.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PulseMotion {
    value: u32,
    from: usize,
    to: usize,
}

struct AppState {
    game: RefCell<Game>,
    store: GameStore,
    persistence_error: RefCell<Option<String>>,
    move_animation_timer: Timer,
    pulse_animation_timer: Timer,
    redraw_timer: Timer,
    pending_direction: Cell<Option<Direction>>,
    active_pulses: RefCell<Vec<MergedTile>>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("SquareMasher failed: {error}");
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let store = GameStore::portable()?;
    let (game, persistence_error) = store.load_or_new();
    let state = Rc::new(AppState {
        game: RefCell::new(game),
        store,
        persistence_error: RefCell::new(persistence_error),
        move_animation_timer: Timer::default(),
        pulse_animation_timer: Timer::default(),
        redraw_timer: Timer::default(),
        pending_direction: Cell::new(None),
        active_pulses: RefCell::new(Vec::new()),
    });
    let window = GameWindow::new()?;
    let tray = AppTray::new()?;
    let icon = app_icon();
    window.set_app_icon(icon.clone());
    tray.set_app_icon(icon);
    sync_ui(&window, &state);

    {
        let state = state.clone();
        window.window().on_close_requested(move || {
            state.redraw_timer.stop();
            CloseRequestResponse::HideWindow
        });
    }

    {
        let weak = window.as_weak();
        let state = state.clone();
        window.on_moved(move |direction| {
            let Some(direction) = parse_direction(direction.as_str()) else {
                return;
            };
            let Some(window) = weak.upgrade() else {
                return;
            };
            request_move(&window, &state, direction);
        });
    }
    {
        let weak = window.as_weak();
        let state = state.clone();
        window.on_reset(move || reset_game(&weak, &state));
    }
    {
        let weak = window.as_weak();
        let state = state.clone();
        window.on_undo(move || undo_game(&weak, &state));
    }
    {
        let weak = window.as_weak();
        let state = state.clone();
        window.on_keep_going(move || keep_going(&weak, &state));
    }
    {
        let weak = window.as_weak();
        let state = state.clone();
        tray.on_open(move || {
            if let Some(window) = weak.upgrade() {
                let _ = window.show();
                start_unlocked_redraw(&window, &state);
            }
        });
    }
    {
        let weak = window.as_weak();
        let state = state.clone();
        tray.on_reset_window_size(move || {
            if let Some(window) = weak.upgrade() {
                let _ = window.show();
                reset_window_size(&window);
                start_unlocked_redraw(&window, &state);
            }
        });
    }
    {
        let state = state.clone();
        tray.on_quit(move || {
            let _ = state.store.save(&state.game.borrow());
            let _ = slint::quit_event_loop();
        });
    }

    window.show()?;
    start_unlocked_redraw(&window, &state);
    tray.show()?;
    slint::run_event_loop()?;
    state.store.save(&state.game.borrow())?;
    Ok(())
}

fn parse_direction(value: &str) -> Option<Direction> {
    match value {
        "left" => Some(Direction::Left),
        "right" => Some(Direction::Right),
        "up" => Some(Direction::Up),
        "down" => Some(Direction::Down),
        _ => None,
    }
}

fn reset_game(window: &slint::Weak<GameWindow>, state: &Rc<AppState>) {
    state.pending_direction.set(None);
    state.move_animation_timer.stop();
    state.pulse_animation_timer.stop();
    if let Some(window) = window.upgrade() {
        finish_move_animation(&window);
        finish_merge_pulse(&window, state);
    }
    state.game.borrow_mut().reset();
    persist_and_sync(window, state);
}

fn undo_game(window: &slint::Weak<GameWindow>, state: &Rc<AppState>) {
    state.pending_direction.set(None);
    state.move_animation_timer.stop();
    state.pulse_animation_timer.stop();
    if let Some(window) = window.upgrade() {
        finish_move_animation(&window);
        finish_merge_pulse(&window, state);
    }
    if state.game.borrow_mut().undo() {
        persist_and_sync(window, state);
    }
}

fn keep_going(window: &slint::Weak<GameWindow>, state: &Rc<AppState>) {
    state.game.borrow_mut().keep_going();
    persist_and_sync(window, state);
}

fn request_move(window: &GameWindow, state: &Rc<AppState>, direction: Direction) {
    if window.get_animating() {
        state.pending_direction.set(Some(direction));
        return;
    }

    {
        let game = state.game.borrow();
        if game.is_over() || game.should_show_clear() {
            return;
        }
    }

    let Some(transition) = state.game.borrow_mut().play_transition(direction) else {
        return;
    };

    persist(state);
    interrupt_moved_merge_pulse(window, state, &transition);
    start_move_animation(window, state, transition);
}

fn start_move_animation(window: &GameWindow, state: &Rc<AppState>, transition: MoveTransition) {
    let MoveTransition { motions, merges } = transition;
    state.move_animation_timer.stop();
    window.set_animating(false);
    window.set_move_progress(0.0);
    let moving_tiles = motions
        .into_iter()
        .map(|motion| MovingTile {
            value: i32::try_from(motion.value).unwrap_or(i32::MAX),
            from_index: motion.from as i32,
            to_index: motion.to as i32,
        })
        .collect::<Vec<_>>();
    window.set_moving_tiles(ModelRc::new(VecModel::from(moving_tiles)));
    window.set_animating(true);
    window.set_move_progress(1.0);

    let weak_window = window.as_weak();
    let weak_state = Rc::downgrade(state);
    state.move_animation_timer.start(
        TimerMode::SingleShot,
        MOVE_ANIMATION_FINISH_DELAY,
        move || {
            let (Some(window), Some(state)) = (weak_window.upgrade(), weak_state.upgrade()) else {
                return;
            };
            finish_move_animation(&window);
            sync_ui(&window, &state);
            if merges.is_empty() {
                run_pending_move(&window, &state);
            } else {
                start_merge_pulse(&window, &state, merges.clone());
                run_pending_move(&window, &state);
            }
        },
    );
}

fn start_merge_pulse(window: &GameWindow, state: &Rc<AppState>, merges: Vec<MergedTile>) {
    state.pulse_animation_timer.stop();
    finish_merge_pulse(window, state);
    let mut pulsing_mask = vec![false; BOARD_TILE_COUNT];
    let pulsing_tiles = merges
        .iter()
        .map(|merge| {
            if let Some(slot) = pulsing_mask.get_mut(merge.index) {
                *slot = true;
            }
            PulsingTile {
                value: i32::try_from(merge.value).unwrap_or(i32::MAX),
                from_index: merge.index as i32,
                to_index: merge.index as i32,
            }
        })
        .collect::<Vec<_>>();
    *state.active_pulses.borrow_mut() = merges;
    window.set_pulsing_tiles(ModelRc::new(VecModel::from(pulsing_tiles)));
    window.set_pulsing_mask(ModelRc::new(VecModel::from(pulsing_mask)));
    window.set_pulsing(true);
    window.set_merge_grow_progress(1.0);

    let weak_window = window.as_weak();
    let weak_state = Rc::downgrade(state);
    state
        .pulse_animation_timer
        .start(TimerMode::SingleShot, MERGE_GROW_FINISH_DELAY, move || {
            let (Some(window), Some(state)) = (weak_window.upgrade(), weak_state.upgrade()) else {
                return;
            };
            window.set_merge_shrink_progress(1.0);

            let weak_window = window.as_weak();
            let weak_state = Rc::downgrade(&state);
            state.pulse_animation_timer.start(
                TimerMode::SingleShot,
                MERGE_SHRINK_FINISH_DELAY,
                move || {
                    let (Some(window), Some(state)) = (weak_window.upgrade(), weak_state.upgrade())
                    else {
                        return;
                    };
                    finish_merge_pulse(&window, &state);
                },
            );
        });
}

fn interrupt_moved_merge_pulse(
    window: &GameWindow,
    state: &Rc<AppState>,
    transition: &MoveTransition,
) {
    if !window.get_pulsing() {
        return;
    }

    let pulse_motions = {
        let active_pulses = state.active_pulses.borrow();
        interrupted_pulse_motions(&active_pulses, transition)
    };
    let Some(pulse_motions) = pulse_motions else {
        return;
    };

    let current_scale = window.get_merge_pulse_scale();
    let pulsing_tiles = pulse_motions
        .into_iter()
        .map(|motion| PulsingTile {
            value: i32::try_from(motion.value).unwrap_or(i32::MAX),
            from_index: motion.from as i32,
            to_index: motion.to as i32,
        })
        .collect::<Vec<_>>();
    state.pulse_animation_timer.stop();
    window.set_pulsing_tiles(ModelRc::new(VecModel::from(pulsing_tiles)));
    window.set_pulse_interrupt_progress(0.0);
    window.set_pulse_interrupt_start_scale(current_scale);
    window.set_pulse_interrupted(true);
    window.set_pulse_interrupt_progress(1.0);

    let weak_window = window.as_weak();
    let weak_state = Rc::downgrade(state);
    state.pulse_animation_timer.start(
        TimerMode::SingleShot,
        MERGE_INTERRUPT_FINISH_DELAY,
        move || {
            let (Some(window), Some(state)) = (weak_window.upgrade(), weak_state.upgrade()) else {
                return;
            };
            finish_merge_pulse(&window, &state);
        },
    );
}

fn interrupted_pulse_motions(
    active_pulses: &[MergedTile],
    transition: &MoveTransition,
) -> Option<Vec<PulseMotion>> {
    let mut affected = false;
    let pulse_motions = active_pulses
        .iter()
        .map(|pulse| {
            let matching_motion = transition
                .motions
                .iter()
                .find(|motion| motion.from == pulse.index);
            let to = matching_motion.map_or(pulse.index, |motion| motion.to);
            if let Some(motion) = matching_motion {
                affected |= motion.from != motion.to
                    || transition
                        .merges
                        .iter()
                        .any(|merge| merge.index == motion.to);
            }
            PulseMotion {
                value: pulse.value,
                from: pulse.index,
                to,
            }
        })
        .collect::<Vec<_>>();

    affected.then_some(pulse_motions)
}

fn finish_move_animation(window: &GameWindow) {
    window.set_animating(false);
    window.set_move_progress(0.0);
    window.set_moving_tiles(ModelRc::new(VecModel::<MovingTile>::from(Vec::new())));
}

fn finish_merge_pulse(window: &GameWindow, state: &AppState) {
    window.set_pulsing(false);
    window.set_merge_grow_progress(0.0);
    window.set_merge_shrink_progress(0.0);
    window.set_pulse_interrupted(false);
    window.set_pulse_interrupt_progress(0.0);
    window.set_pulse_interrupt_start_scale(1.0);
    window.set_pulsing_tiles(ModelRc::new(VecModel::<PulsingTile>::from(Vec::new())));
    window.set_pulsing_mask(ModelRc::new(VecModel::<bool>::from(Vec::new())));
    state.active_pulses.borrow_mut().clear();
}

fn run_pending_move(window: &GameWindow, state: &Rc<AppState>) {
    if let Some(direction) = state.pending_direction.take() {
        request_move(window, state, direction);
    }
}

fn persist(state: &AppState) {
    let result = state.store.save(&state.game.borrow());
    *state.persistence_error.borrow_mut() = result.err();
}

fn persist_and_sync(window: &slint::Weak<GameWindow>, state: &Rc<AppState>) {
    persist(state);
    if let Some(window) = window.upgrade() {
        sync_ui(&window, state);
    }
}

fn sync_ui(window: &GameWindow, state: &AppState) {
    let game = state.game.borrow();
    let tiles = game
        .board()
        .iter()
        .flatten()
        .map(|value| i32::try_from(*value).unwrap_or(i32::MAX))
        .collect::<Vec<_>>();
    window.set_tiles(ModelRc::new(VecModel::from(tiles)));
    window.set_score(game.score().to_string().into());
    window.set_high_score(game.high_score().to_string().into());
    window.set_game_won(game.should_show_clear());
    window.set_game_over(game.is_over());
    window.set_can_undo(game.can_undo());

    let status = if let Some(error) = state.persistence_error.borrow().as_ref() {
        format!("Session is not saved: {error}")
    } else if game.should_show_clear() {
        "2048 reached! Clear the board or keep going.".to_owned()
    } else if game.is_over() {
        "No moves left. Reset to mash again.".to_owned()
    } else if game.has_won() {
        "2048 reached! Keep going.".to_owned()
    } else {
        "Join matching squares to reach 2048.".to_owned()
    };
    window.set_status(status.into());
}

fn start_unlocked_redraw(window: &GameWindow, state: &AppState) {
    let weak_window = window.as_weak();
    state
        .redraw_timer
        .start(TimerMode::Repeated, Duration::ZERO, move || {
            let Some(window) = weak_window.upgrade() else {
                return;
            };
            if window.window().is_visible() {
                window.window().request_redraw();
            }
        });
}

fn reset_window_size(window: &GameWindow) {
    let window = window.window();
    window.set_maximized(false);
    let logical_size = LogicalSize::new(RESET_WINDOW_WIDTH, RESET_WINDOW_HEIGHT);
    window.set_size(logical_size);
    let Some((screen_width, screen_height)) = primary_screen_size() else {
        return;
    };
    let physical_size = logical_size.to_physical(window.scale_factor());
    let x = screen_width.saturating_sub(physical_size.width) / 2;
    let y = screen_height.saturating_sub(physical_size.height) / 2;
    window.set_position(PhysicalPosition::new(x as i32, y as i32));
}

#[cfg(target_os = "windows")]
fn primary_screen_size() -> Option<(u32, u32)> {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetSystemMetrics(index: i32) -> i32;
    }

    let width = unsafe { GetSystemMetrics(0) };
    let height = unsafe { GetSystemMetrics(1) };
    (width > 0 && height > 0).then_some((width as u32, height as u32))
}

#[cfg(target_os = "linux")]
fn primary_screen_size() -> Option<(u32, u32)> {
    use x11rb::connection::Connection;

    let (connection, screen_index) = x11rb::connect(None).ok()?;
    let screen = connection.setup().roots.get(screen_index)?;
    Some((
        u32::from(screen.width_in_pixels),
        u32::from(screen.height_in_pixels),
    ))
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn primary_screen_size() -> Option<(u32, u32)> {
    None
}

fn app_icon() -> Image {
    const SIZE: u32 = 64;
    let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(SIZE, SIZE);
    for (index, pixel) in pixels.make_mut_slice().iter_mut().enumerate() {
        let x = index as u32 % SIZE;
        let y = index as u32 / SIZE;
        let column = (x >= 33) as usize;
        let row = (y >= 33) as usize;
        let gap = (29..=34).contains(&x) || (29..=34).contains(&y);
        let (red, green, blue) = if gap {
            (187, 173, 160)
        } else {
            [
                [(238, 228, 218), (242, 177, 121)],
                [(245, 149, 99), (237, 194, 46)],
            ][row][column]
        };
        *pixel = Rgba8Pixel::new(red, green, blue, 255);
    }
    Image::from_rgba8(pixels)
}

#[cfg(test)]
mod animation_tests {
    use super::*;
    use game::TileMotion;

    #[test]
    fn moving_pulsing_tile_is_interrupted() {
        let transition = MoveTransition {
            motions: vec![TileMotion {
                value: 4,
                from: 5,
                to: 4,
            }],
            merges: Vec::new(),
        };

        assert_eq!(
            interrupted_pulse_motions(&[MergedTile { value: 4, index: 5 }], &transition),
            Some(vec![PulseMotion {
                value: 4,
                from: 5,
                to: 4,
            }])
        );
    }

    #[test]
    fn stationary_pulsing_tile_in_merge_is_interrupted() {
        let transition = MoveTransition {
            motions: vec![
                TileMotion {
                    value: 4,
                    from: 0,
                    to: 0,
                },
                TileMotion {
                    value: 4,
                    from: 1,
                    to: 0,
                },
            ],
            merges: vec![MergedTile { value: 8, index: 0 }],
        };

        assert_eq!(
            interrupted_pulse_motions(&[MergedTile { value: 4, index: 0 }], &transition),
            Some(vec![PulseMotion {
                value: 4,
                from: 0,
                to: 0,
            }])
        );
    }

    #[test]
    fn unrelated_stationary_pulsing_tile_keeps_normal_timing() {
        let transition = MoveTransition {
            motions: vec![
                TileMotion {
                    value: 4,
                    from: 0,
                    to: 0,
                },
                TileMotion {
                    value: 2,
                    from: 5,
                    to: 4,
                },
            ],
            merges: Vec::new(),
        };

        assert_eq!(
            interrupted_pulse_motions(&[MergedTile { value: 4, index: 0 }], &transition),
            None
        );
    }

    #[test]
    fn all_active_pulses_follow_transition_when_one_moves() {
        let transition = MoveTransition {
            motions: vec![
                TileMotion {
                    value: 4,
                    from: 0,
                    to: 0,
                },
                TileMotion {
                    value: 8,
                    from: 5,
                    to: 4,
                },
            ],
            merges: Vec::new(),
        };

        assert_eq!(
            interrupted_pulse_motions(
                &[
                    MergedTile { value: 4, index: 0 },
                    MergedTile { value: 8, index: 5 },
                ],
                &transition,
            ),
            Some(vec![
                PulseMotion {
                    value: 4,
                    from: 0,
                    to: 0,
                },
                PulseMotion {
                    value: 8,
                    from: 5,
                    to: 4,
                },
            ])
        );
    }
}
