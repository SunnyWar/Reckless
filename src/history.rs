use std::sync::atomic::{AtomicI16, Ordering};

use crate::{
    numa::NumaValue,
    types::{Bitboard, Color, Move, Piece, PieceType, Square},
};

type FromToHistory<T> = [[T; 64]; 64];
type PieceToHistory<T> = [[T; 64]; 13];
type ContinuationHistoryType = [[[[PieceToHistory<i16>; 64]; 13]; 2]; 2];

const CONT_CAPTURE_DIM: usize = 2;
const CONT_PIECE_DIM: usize = 13;
const CONT_SQUARE_DIM: usize = 64;
const CONT_SUBTABLE_LEN: usize = CONT_PIECE_DIM * CONT_SQUARE_DIM;

fn apply_bonus<const MAX: i32>(entry: &mut i16, bonus: i32) {
    let bonus = bonus.clamp(-MAX, MAX);
    *entry += (bonus - bonus.abs() * (*entry) as i32 / MAX) as i16;
}

struct QuietHistoryEntry {
    factorizer: i16,
    buckets: [[i16; 2]; 2],
}

impl QuietHistoryEntry {
    const MAX_FACTORIZER: i32 = 1852;
    const MAX_BUCKET: i32 = 6324;

    pub const fn bucket(&self, threats: Bitboard, mv: Move) -> i16 {
        let from_threatened = threats.contains(mv.from()) as usize;
        let to_threatened = threats.contains(mv.to()) as usize;

        self.buckets[from_threatened][to_threatened]
    }

    pub fn update_factorizer(&mut self, bonus: i32) {
        let entry = &mut self.factorizer;
        apply_bonus::<{ Self::MAX_FACTORIZER }>(entry, bonus);
    }

    pub fn update_bucket(&mut self, threats: Bitboard, mv: Move, bonus: i32) {
        let from_threatened = threats.contains(mv.from()) as usize;
        let to_threatened = threats.contains(mv.to()) as usize;

        let entry = &mut self.buckets[from_threatened][to_threatened];
        apply_bonus::<{ Self::MAX_BUCKET }>(entry, bonus);
    }
}

pub struct QuietHistory {
    entries: Box<[FromToHistory<QuietHistoryEntry>; 2]>,
}

impl QuietHistory {
    pub fn get(&self, threats: Bitboard, stm: Color, mv: Move) -> i32 {
        let entry = &self.entries[stm][mv.from()][mv.to()];
        (entry.factorizer + entry.bucket(threats, mv)) as i32
    }

    pub fn update(&mut self, threats: Bitboard, stm: Color, mv: Move, bonus: i32) {
        let entry = &mut self.entries[stm][mv.from()][mv.to()];

        entry.update_factorizer(bonus);
        entry.update_bucket(threats, mv, bonus);
    }
}

impl Default for QuietHistory {
    fn default() -> Self {
        Self { entries: zeroed_box() }
    }
}

struct NoisyHistoryEntry {
    factorizer: i16,
    buckets: [[i16; 2]; 7],
}

impl NoisyHistoryEntry {
    const MAX_FACTORIZER: i32 = 4524;
    const MAX_BUCKET: i32 = 7826;

    pub fn bucket(&self, threats: Bitboard, sq: Square, captured: PieceType) -> i16 {
        let threatened = threats.contains(sq) as usize;
        self.buckets[captured][threatened]
    }

    pub fn update_factorizer(&mut self, bonus: i32) {
        let entry = &mut self.factorizer;
        apply_bonus::<{ Self::MAX_FACTORIZER }>(entry, bonus);
    }

    pub fn update_bucket(&mut self, threats: Bitboard, sq: Square, captured: PieceType, bonus: i32) {
        let threatened = threats.contains(sq) as usize;
        let entry = &mut self.buckets[captured][threatened];
        apply_bonus::<{ Self::MAX_BUCKET }>(entry, bonus);
    }
}

pub struct NoisyHistory {
    // [piece][to][captured_piece_type][to_threatened]
    entries: Box<PieceToHistory<NoisyHistoryEntry>>,
}

impl NoisyHistory {
    pub fn get(&self, threats: Bitboard, piece: Piece, sq: Square, captured: PieceType) -> i32 {
        let entry = &self.entries[piece][sq];
        (entry.factorizer + entry.bucket(threats, sq, captured)) as i32
    }

    pub fn update(&mut self, threats: Bitboard, piece: Piece, sq: Square, captured: PieceType, bonus: i32) {
        let entry = &mut self.entries[piece][sq];

        entry.update_factorizer(bonus);
        entry.update_bucket(threats, sq, captured, bonus);
    }
}

impl Default for NoisyHistory {
    fn default() -> Self {
        Self { entries: zeroed_box() }
    }
}

pub struct CorrectionHistory {
    // [side_to_move][key]
    entries: Box<[[AtomicI16; Self::SIZE]; 2]>,
}

unsafe impl NumaValue for CorrectionHistory {}

impl CorrectionHistory {
    const MAX_HISTORY: i32 = 14605;

    const SIZE: usize = 65536;
    const MASK: usize = Self::SIZE - 1;

    pub fn get(&self, stm: Color, key: u64) -> i32 {
        self.entries[stm][key as usize & Self::MASK].load(Ordering::Relaxed) as i32
    }

    pub fn update(&self, stm: Color, key: u64, bonus: i32) {
        let current = self.entries[stm][key as usize & Self::MASK].load(Ordering::Relaxed) as i32;
        let new = current + bonus - bonus.abs() * current / Self::MAX_HISTORY;
        self.entries[stm][key as usize & Self::MASK].store(new as i16, Ordering::Relaxed);
    }

    pub fn clear(&self) {
        for entries in self.entries.iter() {
            for entry in entries {
                entry.store(0, Ordering::Relaxed);
            }
        }
    }
}

impl Default for CorrectionHistory {
    fn default() -> Self {
        Self { entries: zeroed_box() }
    }
}

pub struct ContinuationCorrectionHistory {
    // [in_check][capture][piece][to][piece][to]
    entries: Box<ContinuationHistoryType>,
}

impl ContinuationCorrectionHistory {
    const MAX_HISTORY: i32 = 16282;

    pub fn get(&self, key: ContinuationKey, sub_piece: Piece, sub_square: Square) -> i32 {
        continuation_history_get(self, key, sub_piece, sub_square)
    }

    pub fn update(&mut self, key: ContinuationKey, sub_piece: Piece, sub_square: Square, bonus: i32) {
        continuation_history_update(self, key, sub_piece, sub_square, bonus);
    }
}

impl ContHistory for ContinuationCorrectionHistory {
    const MAX_HISTORY: i32 = Self::MAX_HISTORY;

    fn history_entry(&self, key: ContinuationKey) -> &PieceToHistory<i16> {
        let (in_check, is_capture, piece, square) = key.decode();
        unsafe {
            self.entries
                .as_ref()
                .get_unchecked(in_check)
                .get_unchecked(is_capture)
                .get_unchecked(piece)
                .get_unchecked(square)
        }
    }

    fn history_entry_mut(&mut self, key: ContinuationKey) -> &mut PieceToHistory<i16> {
        let (in_check, is_capture, piece, square) = key.decode();
        unsafe {
            self.entries
                .as_mut()
                .get_unchecked_mut(in_check)
                .get_unchecked_mut(is_capture)
                .get_unchecked_mut(piece)
                .get_unchecked_mut(square)
        }
    }
}

impl Default for ContinuationCorrectionHistory {
    fn default() -> Self {
        Self { entries: zeroed_box() }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ContinuationKey {
    pub offset: usize,
}

impl ContinuationKey {
    pub const fn outer_index(in_check: bool, is_capture: bool, piece: Piece, square: Square) -> usize {
        (((in_check as usize * CONT_CAPTURE_DIM + is_capture as usize) * CONT_PIECE_DIM + piece as usize)
            * CONT_SQUARE_DIM)
            + square as usize
    }

    pub const fn from_parts(in_check: bool, is_capture: bool, piece: Piece, square: Square) -> Self {
        Self {
            offset: Self::outer_index(in_check, is_capture, piece, square) * CONT_SUBTABLE_LEN,
        }
    }

    fn decode(self) -> (usize, usize, usize, usize) {
        let mut idx = self.offset / CONT_SUBTABLE_LEN;

        let square = idx % CONT_SQUARE_DIM;
        idx /= CONT_SQUARE_DIM;

        let piece = idx % CONT_PIECE_DIM;
        idx /= CONT_PIECE_DIM;

        let is_capture = idx % CONT_CAPTURE_DIM;
        let in_check = idx / CONT_CAPTURE_DIM;

        (in_check, is_capture, piece, square)
    }
}

impl Default for ContinuationKey {
    fn default() -> Self {
        Self::from_parts(false, false, Piece::None, Square::A1)
    }
}

trait ContHistory {
    const MAX_HISTORY: i32;

    fn history_entry(&self, key: ContinuationKey) -> &PieceToHistory<i16>;
    fn history_entry_mut(&mut self, key: ContinuationKey) -> &mut PieceToHistory<i16>;
}

fn continuation_history_get<T: ContHistory>(
    history: &T, key: ContinuationKey, sub_piece: Piece, sub_square: Square,
) -> i32 {
    unsafe { *history.history_entry(key).get_unchecked(sub_piece as usize).get_unchecked(sub_square as usize) as i32 }
}

fn continuation_history_update<T: ContHistory>(
    history: &mut T, key: ContinuationKey, sub_piece: Piece, sub_square: Square, bonus: i32,
) {
    let entry = unsafe {
        history.history_entry_mut(key).get_unchecked_mut(sub_piece as usize).get_unchecked_mut(sub_square as usize)
    };
    *entry += (bonus - bonus.abs() * (*entry) as i32 / T::MAX_HISTORY) as i16;
}

pub struct ContinuationHistory {
    // [in_check][capture][piece][to][piece][to]
    entries: Box<ContinuationHistoryType>,
}

impl ContinuationHistory {
    const MAX_HISTORY: i32 = 15168;

    pub fn get(&self, key: ContinuationKey, sub_piece: Piece, sub_square: Square) -> i32 {
        continuation_history_get(self, key, sub_piece, sub_square)
    }

    pub fn update(&mut self, key: ContinuationKey, sub_piece: Piece, sub_square: Square, bonus: i32) {
        continuation_history_update(self, key, sub_piece, sub_square, bonus);
    }
}

impl ContHistory for ContinuationHistory {
    const MAX_HISTORY: i32 = Self::MAX_HISTORY;

    fn history_entry(&self, key: ContinuationKey) -> &PieceToHistory<i16> {
        let (in_check, is_capture, piece, square) = key.decode();
        unsafe {
            self.entries
                .as_ref()
                .get_unchecked(in_check)
                .get_unchecked(is_capture)
                .get_unchecked(piece)
                .get_unchecked(square)
        }
    }

    fn history_entry_mut(&mut self, key: ContinuationKey) -> &mut PieceToHistory<i16> {
        let (in_check, is_capture, piece, square) = key.decode();
        unsafe {
            self.entries
                .as_mut()
                .get_unchecked_mut(in_check)
                .get_unchecked_mut(is_capture)
                .get_unchecked_mut(piece)
                .get_unchecked_mut(square)
        }
    }
}

impl Default for ContinuationHistory {
    fn default() -> Self {
        Self { entries: zeroed_box() }
    }
}

fn zeroed_box<T>() -> Box<T> {
    unsafe {
        let layout = std::alloc::Layout::new::<T>();
        let ptr = std::alloc::alloc_zeroed(layout);
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Box::<T>::from_raw(ptr.cast())
    }
}
