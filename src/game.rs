use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const BOARD_SIZE: usize = 4;
const SAVE_VERSION: u32 = 1;
const MAX_SAVE_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileMotion {
    pub value: u32,
    pub from: usize,
    pub to: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MergedTile {
    pub value: u32,
    pub index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveTransition {
    pub motions: Vec<TileMotion>,
    pub merges: Vec<MergedTile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Game {
    board: [[u32; BOARD_SIZE]; BOARD_SIZE],
    score: u64,
    high_score: u64,
    #[serde(default)]
    clear_acknowledged: bool,
    #[serde(skip)]
    undo: Option<UndoState>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct UndoState {
    board: [[u32; BOARD_SIZE]; BOARD_SIZE],
    score: u64,
    clear_acknowledged: bool,
}

#[derive(Serialize, Deserialize)]
struct SaveFile {
    version: u32,
    game: Game,
}

pub struct GameStore {
    path: PathBuf,
    _instance_lock: Option<File>,
}

impl Game {
    pub fn new() -> Self {
        let mut game = Self {
            board: [[0; BOARD_SIZE]; BOARD_SIZE],
            score: 0,
            high_score: 0,
            clear_acknowledged: false,
            undo: None,
        };
        game.spawn_tile();
        game.spawn_tile();
        game
    }

    pub fn board(&self) -> &[[u32; BOARD_SIZE]; BOARD_SIZE] {
        &self.board
    }

    pub fn score(&self) -> u64 {
        self.score
    }

    pub fn high_score(&self) -> u64 {
        self.high_score
    }

    #[cfg(test)]
    pub fn play(&mut self, direction: Direction) -> bool {
        self.play_transition(direction).is_some()
    }

    pub fn play_transition(&mut self, direction: Direction) -> Option<MoveTransition> {
        let undo = UndoState {
            board: self.board,
            score: self.score,
            clear_acknowledged: self.clear_acknowledged,
        };
        let transition = self.slide_transition(direction)?;
        self.spawn_tile();
        self.undo = Some(undo);
        Some(transition)
    }

    pub fn reset(&mut self) {
        self.board = [[0; BOARD_SIZE]; BOARD_SIZE];
        self.score = 0;
        self.clear_acknowledged = false;
        self.undo = None;
        self.spawn_tile();
        self.spawn_tile();
    }

    pub fn has_won(&self) -> bool {
        self.board.iter().flatten().any(|tile| *tile >= 2048)
    }

    pub fn should_show_clear(&self) -> bool {
        self.has_won() && !self.clear_acknowledged
    }

    pub fn keep_going(&mut self) {
        if self.has_won() {
            self.clear_acknowledged = true;
        }
    }

    pub fn can_undo(&self) -> bool {
        self.undo.is_some()
    }

    pub fn undo(&mut self) -> bool {
        let Some(undo) = self.undo.take() else {
            return false;
        };
        self.board = undo.board;
        self.score = undo.score;
        self.clear_acknowledged = undo.clear_acknowledged;
        true
    }

    pub fn is_over(&self) -> bool {
        for row in 0..BOARD_SIZE {
            for column in 0..BOARD_SIZE {
                let tile = self.board[row][column];
                if tile == 0
                    || row + 1 < BOARD_SIZE && self.board[row + 1][column] == tile
                    || column + 1 < BOARD_SIZE && self.board[row][column + 1] == tile
                {
                    return false;
                }
            }
        }

        true
    }

    #[cfg(test)]
    fn slide(&mut self, direction: Direction) -> bool {
        self.slide_transition(direction).is_some()
    }

    #[cfg(test)]
    fn slide_with_motions(&mut self, direction: Direction) -> Vec<TileMotion> {
        self.slide_transition(direction)
            .map_or_else(Vec::new, |transition| transition.motions)
    }

    fn slide_transition(&mut self, direction: Direction) -> Option<MoveTransition> {
        let before = self.board;
        let mut motions = Vec::new();
        let mut merges = Vec::new();

        for line in 0..BOARD_SIZE {
            let mut compact = [(0, 0); BOARD_SIZE];
            let mut count = 0;

            for offset in 0..BOARD_SIZE {
                let (row, column) = coordinates(direction, line, offset);
                let value = self.board[row][column];
                if value != 0 {
                    compact[count] = (value, offset);
                    count += 1;
                }
            }

            let mut merged = [0; BOARD_SIZE];
            let mut read = 0;
            let mut write = 0;
            while read < count {
                let (value, source_offset) = compact[read];
                let (to_row, to_column) = coordinates(direction, line, write);
                if read + 1 < count && value == compact[read + 1].0 {
                    let value = value.saturating_mul(2);
                    merged[write] = value;
                    self.score = self.score.saturating_add(u64::from(value));
                    merges.push(MergedTile {
                        value,
                        index: to_row * BOARD_SIZE + to_column,
                    });
                    for source_offset in [source_offset, compact[read + 1].1] {
                        let (from_row, from_column) = coordinates(direction, line, source_offset);
                        motions.push(TileMotion {
                            value: compact[read].0,
                            from: from_row * BOARD_SIZE + from_column,
                            to: to_row * BOARD_SIZE + to_column,
                        });
                    }
                    read += 2;
                } else {
                    merged[write] = value;
                    let (from_row, from_column) = coordinates(direction, line, source_offset);
                    motions.push(TileMotion {
                        value,
                        from: from_row * BOARD_SIZE + from_column,
                        to: to_row * BOARD_SIZE + to_column,
                    });
                    read += 1;
                }
                write += 1;
            }

            for (offset, value) in merged.into_iter().enumerate() {
                let (row, column) = coordinates(direction, line, offset);
                self.board[row][column] = value;
            }
        }

        if self.board == before {
            return None;
        }

        self.high_score = self.high_score.max(self.score);
        Some(MoveTransition { motions, merges })
    }

    fn spawn_tile(&mut self) {
        let empty = self
            .board
            .iter()
            .enumerate()
            .flat_map(|(row, values)| {
                values
                    .iter()
                    .enumerate()
                    .filter_map(move |(column, value)| (*value == 0).then_some((row, column)))
            })
            .collect::<Vec<_>>();

        if empty.is_empty() {
            return;
        }

        let (row, column) = empty[fastrand::usize(..empty.len())];
        self.board[row][column] = if fastrand::u8(..10) == 0 { 4 } else { 2 };
    }

    fn is_valid(&self) -> bool {
        self.high_score >= self.score
            && self
                .board
                .iter()
                .flatten()
                .filter(|tile| **tile != 0)
                .count()
                >= 2
            && self
                .board
                .iter()
                .flatten()
                .all(|tile| *tile == 0 || tile.is_power_of_two())
    }
}

impl GameStore {
    pub fn portable() -> Result<Self, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("could not locate executable: {error}"))?;
        let directory = executable
            .parent()
            .ok_or_else(|| "executable has no parent directory".to_owned())?;
        let lock_path = directory.join("SquareMasher.lock");
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| format!("could not open {}: {error}", lock_path.display()))?;
        lock.try_lock().map_err(|error| {
            format!("another SquareMasher instance may already be running: {error}")
        })?;
        Ok(Self {
            path: directory.join("SquareMasher.json"),
            _instance_lock: Some(lock),
        })
    }

    pub fn load_or_new(&self) -> (Game, Option<String>) {
        match self.load() {
            Ok(Some(game)) => (game, None),
            Ok(None) => (Game::new(), None),
            Err(error) => (Game::new(), Some(error)),
        }
    }

    pub fn save(&self, game: &Game) -> Result<(), String> {
        let data = serde_json::to_vec_pretty(&SaveFile {
            version: SAVE_VERSION,
            game: game.clone(),
        })
        .map_err(|error| format!("could not encode save: {error}"))?;
        let temporary = temporary_path(&self.path);
        let mut file = File::create(&temporary)
            .map_err(|error| format!("could not create {}: {error}", temporary.display()))?;
        file.write_all(&data)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;

        match fs::rename(&temporary, &self.path) {
            Ok(()) => Ok(()),
            Err(_error) if self.path.exists() => {
                fs::remove_file(&self.path).map_err(|remove_error| {
                    format!("could not replace {}: {remove_error}", self.path.display())
                })?;
                fs::rename(&temporary, &self.path).map_err(|rename_error| {
                    format!("could not replace {}: {rename_error}", self.path.display())
                })
            }
            Err(error) => Err(format!("could not save {}: {error}", self.path.display())),
        }
    }

    #[cfg(test)]
    fn at(path: PathBuf) -> Self {
        Self {
            path,
            _instance_lock: None,
        }
    }

    fn load(&self) -> Result<Option<Game>, String> {
        let mut first_error = None;
        for candidate in [temporary_path(&self.path), self.path.clone()] {
            match read_game(&candidate) {
                Ok(Some(game)) => return Ok(Some(game)),
                Ok(None) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            };
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(None),
        }
    }
}

fn coordinates(direction: Direction, line: usize, offset: usize) -> (usize, usize) {
    match direction {
        Direction::Left => (line, offset),
        Direction::Right => (line, BOARD_SIZE - 1 - offset),
        Direction::Up => (offset, line),
        Direction::Down => (BOARD_SIZE - 1 - offset, line),
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(".tmp");
    PathBuf::from(name)
}

fn read_game(path: &Path) -> Result<Option<Game>, String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not inspect {}: {error}", path.display())),
    };
    if metadata.len() > MAX_SAVE_BYTES {
        return Err(format!("{} is too large to be a save file", path.display()));
    }

    let data =
        fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let saved: SaveFile = serde_json::from_slice(&data)
        .map_err(|error| format!("could not parse {}: {error}", path.display()))?;
    if saved.version != SAVE_VERSION || !saved.game.is_valid() {
        return Err(format!("{} contains invalid game data", path.display()));
    }

    Ok(Some(saved.game))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(board: [[u32; BOARD_SIZE]; BOARD_SIZE]) -> Game {
        Game {
            board,
            score: 0,
            high_score: 0,
            clear_acknowledged: false,
            undo: None,
        }
    }

    fn nonzero_tiles(game: &Game) -> usize {
        game.board
            .iter()
            .flatten()
            .filter(|tile| **tile != 0)
            .count()
    }

    fn test_store(name: &str) -> (PathBuf, GameStore) {
        let directory = std::env::temp_dir().join(format!(
            "square-masher-{name}-{}-{}",
            std::process::id(),
            fastrand::u64(..)
        ));
        fs::create_dir_all(&directory).expect("create test directory");
        let store = GameStore::at(directory.join("SquareMasher.json"));
        (directory, store)
    }

    #[test]
    fn merges_each_pair_once_and_scores() {
        let mut game = game([[2, 2, 2, 2], [2, 2, 4, 0], [4, 4, 8, 8], [0, 0, 0, 0]]);

        assert!(game.slide(Direction::Left));
        assert_eq!(game.board[0], [4, 4, 0, 0]);
        assert_eq!(game.board[1], [4, 4, 0, 0]);
        assert_eq!(game.board[2], [8, 16, 0, 0]);
        assert_eq!(game.score, 36);
        assert_eq!(game.high_score, 36);
    }

    #[test]
    fn reports_each_source_tile_motion_for_merges() {
        let mut left = game([[0, 2, 2, 4], [0; 4], [0; 4], [0; 4]]);
        assert_eq!(
            left.slide_with_motions(Direction::Left),
            vec![
                TileMotion {
                    value: 2,
                    from: 1,
                    to: 0,
                },
                TileMotion {
                    value: 2,
                    from: 2,
                    to: 0,
                },
                TileMotion {
                    value: 4,
                    from: 3,
                    to: 1,
                },
            ]
        );
        assert_eq!(left.board[0], [4, 4, 0, 0]);

        let mut right = game([[2, 2, 4, 4], [0; 4], [0; 4], [0; 4]]);
        assert_eq!(
            right.slide_with_motions(Direction::Right),
            vec![
                TileMotion {
                    value: 4,
                    from: 3,
                    to: 3,
                },
                TileMotion {
                    value: 4,
                    from: 2,
                    to: 3,
                },
                TileMotion {
                    value: 2,
                    from: 1,
                    to: 2,
                },
                TileMotion {
                    value: 2,
                    from: 0,
                    to: 2,
                },
            ]
        );
        assert_eq!(right.board[0], [0, 0, 4, 8]);

        let mut down = game([[2, 0, 0, 0], [0; 4], [2, 0, 0, 0], [4, 0, 0, 0]]);
        assert_eq!(
            down.slide_with_motions(Direction::Down),
            vec![
                TileMotion {
                    value: 4,
                    from: 12,
                    to: 12,
                },
                TileMotion {
                    value: 2,
                    from: 8,
                    to: 8,
                },
                TileMotion {
                    value: 2,
                    from: 0,
                    to: 8,
                },
            ]
        );
        assert_eq!(down.board.map(|row| row[0]), [0, 0, 4, 4]);
    }

    #[test]
    fn reports_only_merged_destination_tiles() {
        let mut left = game([[2, 2, 2, 2], [0; 4], [0; 4], [0; 4]]);
        assert_eq!(
            left.slide_transition(Direction::Left)
                .expect("left move")
                .merges,
            vec![
                MergedTile { value: 4, index: 0 },
                MergedTile { value: 4, index: 1 },
            ]
        );

        let mut right = game([[2, 2, 4, 4], [0; 4], [0; 4], [0; 4]]);
        assert_eq!(
            right
                .slide_transition(Direction::Right)
                .expect("right move")
                .merges,
            vec![
                MergedTile { value: 8, index: 3 },
                MergedTile { value: 4, index: 2 },
            ]
        );

        let mut down = game([[2, 0, 0, 0], [2, 0, 0, 0], [4, 0, 0, 0], [4, 0, 0, 0]]);
        assert_eq!(
            down.slide_transition(Direction::Down)
                .expect("down move")
                .merges,
            vec![
                MergedTile {
                    value: 8,
                    index: 12,
                },
                MergedTile { value: 4, index: 8 },
            ]
        );

        let mut movement_only = game([[0, 0, 0, 2], [0; 4], [0; 4], [0; 4]]);
        assert!(
            movement_only
                .slide_transition(Direction::Left)
                .expect("movement")
                .merges
                .is_empty()
        );
    }

    #[test]
    fn moves_in_all_four_directions() {
        let mut right = game([[2, 0, 0, 0], [0; 4], [0; 4], [0; 4]]);
        assert!(right.slide(Direction::Right));
        assert_eq!(right.board[0], [0, 0, 0, 2]);

        let mut up = game([[0; 4], [0; 4], [0; 4], [0, 2, 0, 0]]);
        assert!(up.slide(Direction::Up));
        assert_eq!(up.board[0][1], 2);

        let mut down = game([[0, 0, 2, 0], [0; 4], [0; 4], [0; 4]]);
        assert!(down.slide(Direction::Down));
        assert_eq!(down.board[3][2], 2);

        let mut left = game([[0, 0, 0, 2], [0; 4], [0; 4], [0; 4]]);
        assert!(left.slide(Direction::Left));
        assert_eq!(left.board[0], [2, 0, 0, 0]);

        let mut merge_right = game([[2, 2, 4, 4], [0; 4], [0; 4], [0; 4]]);
        assert!(merge_right.slide(Direction::Right));
        assert_eq!(merge_right.board[0], [0, 0, 4, 8]);

        let mut merge_down = game([[2, 0, 0, 0], [2, 0, 0, 0], [4, 0, 0, 0], [4, 0, 0, 0]]);
        assert!(merge_down.slide(Direction::Down));
        assert_eq!(merge_down.board.map(|row| row[0]), [0, 0, 4, 8]);
    }

    #[test]
    fn only_effective_moves_spawn() {
        let mut blocked = game([[2, 0, 0, 0], [0; 4], [0; 4], [0; 4]]);
        let before = blocked.clone();
        assert!(!blocked.play(Direction::Left));
        assert_eq!(blocked, before);

        let mut movable = game([[0, 2, 0, 0], [0; 4], [0; 4], [0; 4]]);
        assert!(movable.play(Direction::Left));
        assert_eq!(nonzero_tiles(&movable), 2);
    }

    #[test]
    fn undo_restores_board_and_score_without_lowering_high_score() {
        let mut game = game([[2, 2, 0, 0], [0; 4], [0; 4], [0; 4]]);
        let before = game.clone();

        assert!(game.play(Direction::Left));
        assert!(game.can_undo());
        assert_eq!(game.score, 4);
        assert_eq!(game.high_score, 4);

        assert!(game.undo());
        assert_eq!(game.board, before.board);
        assert_eq!(game.score, before.score);
        assert_eq!(game.high_score, 4);
        assert!(!game.can_undo());
        assert!(!game.undo());
    }

    #[test]
    fn detects_game_over_and_remaining_merge() {
        let over = game([[2, 4, 2, 4], [4, 2, 4, 2], [2, 4, 2, 4], [4, 2, 4, 2]]);
        assert!(over.is_over());

        let space_remaining = game([[2, 4, 2, 4], [4, 2, 4, 2], [2, 4, 0, 2], [4, 2, 4, 2]]);
        assert!(!space_remaining.is_over());

        let merge_left = game([
            [2, 2, 4, 8],
            [4, 8, 16, 32],
            [8, 16, 32, 64],
            [16, 32, 64, 128],
        ]);
        assert!(!merge_left.is_over());
    }

    #[test]
    fn reset_preserves_high_score() {
        let mut game = game([[2, 2, 0, 0], [0; 4], [0; 4], [0; 4]]);
        game.slide(Direction::Left);
        assert_eq!(game.high_score, 4);

        game.reset();
        assert_eq!(game.score, 0);
        assert_eq!(game.high_score, 4);
        assert_eq!(nonzero_tiles(&game), 2);
    }

    #[test]
    fn clear_requires_acknowledgement_and_reset_restores_it() {
        let mut game = game([[1024, 1024, 0, 0], [0; 4], [0; 4], [0; 4]]);

        assert!(game.slide(Direction::Left));
        assert!(game.has_won());
        assert!(game.should_show_clear());

        game.keep_going();
        assert!(!game.should_show_clear());

        game.reset();
        assert!(!game.clear_acknowledged);
    }

    #[test]
    fn save_round_trip_restores_exact_session() {
        let (directory, store) = test_store("round-trip");
        let mut expected = game([[2, 4, 8, 16], [0; 4], [0; 4], [0; 4]]);
        expected.score = 48;
        expected.high_score = 128;

        store.save(&expected).expect("save game");
        let (actual, error) = store.load_or_new();
        assert_eq!(error, None);
        assert_eq!(actual, expected);

        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn legacy_session_without_clear_acknowledgement_loads() {
        let (directory, store) = test_store("legacy-clear");
        fs::write(
            &store.path,
            br#"{"version":1,"game":{"board":[[2,4,0,0],[0,0,0,0],[0,0,0,0],[0,0,0,0]],"score":0,"high_score":0}}"#,
        )
        .expect("write legacy save");

        let (game, error) = store.load_or_new();
        assert_eq!(error, None);
        assert!(!game.should_show_clear());

        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn later_save_replaces_earlier_session() {
        let (directory, store) = test_store("overwrite");
        let first = game([[2, 4, 0, 0], [0; 4], [0; 4], [0; 4]]);
        let mut expected = game([[8, 16, 0, 0], [0; 4], [0; 4], [0; 4]]);
        expected.score = 24;
        expected.high_score = 24;

        store.save(&first).expect("save first game");
        store.save(&expected).expect("replace first game");
        let (actual, error) = store.load_or_new();
        assert_eq!(error, None);
        assert_eq!(actual, expected);
        assert!(!temporary_path(&store.path).exists());

        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn malformed_save_starts_fresh_without_panicking() {
        let (directory, store) = test_store("malformed");
        fs::write(&store.path, b"not json").expect("write malformed save");

        let (game, error) = store.load_or_new();
        assert!(error.is_some());
        assert_eq!(nonzero_tiles(&game), 2);

        fs::remove_dir_all(directory).expect("remove test directory");
    }
}
