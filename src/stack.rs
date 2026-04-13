use std::ops::{Index, IndexMut};

use crate::history::ContinuationKey;
use crate::types::{MAX_PLY, Move, Piece, Score};

pub struct Stack {
    data: [StackEntry; MAX_PLY + 16],
}

impl Stack {
    pub fn sentinel(&mut self) -> &mut StackEntry {
        unsafe { self.data.get_unchecked_mut(0) }
    }

    pub fn new() -> Box<Self> {
        let mut stack = Box::new(Self::default());
        for entry in &mut stack.data {
            entry.conthist = ContinuationKey::default();
            entry.contcorrhist = ContinuationKey::default();
        }
        stack
    }
}

impl Default for Stack {
    fn default() -> Self {
        Self { data: [StackEntry::default(); MAX_PLY + 16] }
    }
}

#[derive(Copy, Clone)]
pub struct StackEntry {
    pub mv: Move,
    pub piece: Piece,
    pub eval: i32,
    pub excluded: Move,
    pub tt_move: Move,
    pub tt_pv: bool,
    pub cutoff_count: i32,
    pub move_count: i32,
    pub reduction: i32,
    pub conthist: ContinuationKey,
    pub contcorrhist: ContinuationKey,
}

unsafe impl Send for StackEntry {}

impl Default for StackEntry {
    fn default() -> Self {
        Self {
            mv: Move::NULL,
            piece: Piece::None,
            eval: Score::NONE,
            excluded: Move::NULL,
            tt_move: Move::NULL,
            tt_pv: false,
            cutoff_count: 0,
            move_count: 0,
            reduction: 0,
            conthist: ContinuationKey::default(),
            contcorrhist: ContinuationKey::default(),
        }
    }
}

impl Index<isize> for Stack {
    type Output = StackEntry;

    fn index(&self, index: isize) -> &Self::Output {
        debug_assert!(index + 8 >= 0 && index < MAX_PLY as isize + 16);
        &self.data[(index + 8) as usize]
    }
}

impl IndexMut<isize> for Stack {
    fn index_mut(&mut self, index: isize) -> &mut Self::Output {
        debug_assert!(index + 8 >= 0 && index < MAX_PLY as isize + 16);
        &mut self.data[(index + 8) as usize]
    }
}
